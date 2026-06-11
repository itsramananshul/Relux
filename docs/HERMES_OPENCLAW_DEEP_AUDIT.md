# Hermes / OpenClaw / Paperclip — deep segmented audit vs Relux

> **Status: durable engineering map.** This is the mechanism-level companion to the
> ideas-only [`hermes-vs-paperclip-vs-relix.md`](hermes-vs-paperclip-vs-relix.md) and the
> per-slice ledger [`reference-driven-development.md`](reference-driven-development.md). It
> exists so a future Claude/Codex run can pick the next agentic/product slice WITHOUT
> re-reading three reference codebases from scratch. It is segmented into 12 dimensions; each
> is audited independently with (a) the reference mechanism + exact files, (b) the Relux
> mapping marked **implemented / partial / missing** with exact Relux files, and (c) a
> priority (P0/P1/P2) + concrete implementation slices + which surfaces they touch
> (backend / frontend / docs / tests).
>
> **Binding context.** Per [`CLAUDE.md`](../CLAUDE.md) and
> [`reference-driven-development.md`](reference-driven-development.md), the reference clones
> are read-only design sources, never copied verbatim. The Relux safety spine is non-negotiable:
> **the brain proposes; the deterministic kernel is the sole authority; every durable change
> flows through `decide` → `prime_execute` / human approval.** Nothing in this audit weakens
> that — slices that would give the brain new authority are explicitly called out and deferred.

## Reference folders read (the real paths in this repo)

- **Hermes** (Python): `reference/hermes-agent-main/` (binding mirror) and `references/hermes-agent/`
  (newer, v0.15). Same source tree; cite `reference/hermes-agent-main/` to match repo convention.
- **OpenClaw** (TypeScript): `reference/openclaw-main/` (`src/…`, `packages/…`).
- **Paperclip** (TypeScript): `references/paperclip/` (`server/src/…`, `packages/…`, `ui/…`).
- **open-webui** (UI analogue): `reference/open-webui-main/` (consulted for UI ergonomics only).

Relux roots audited: `crates/relux-core/src/`, `crates/relux-kernel/src/`, `apps/dashboard/`.

## How to read the status column

- **implemented** — Relux has a working, doc-conformant mechanism at parity (for Relux's threat
  model / scope) with the reference. Not necessarily identical; equivalent.
- **partial** — a real mechanism exists but is materially narrower than the references.
- **missing** — no equivalent exists (and is either a genuine gap or a deliberate non-goal).

---

## Top P0/P1 gaps (the executive summary)

| # | Gap | Dim | Priority | Surfaces |
|---|-----|-----|----------|----------|
| 1 | **Self-correction on a malformed brain decision** — a correctable reply is collapsed into the same `None` as a hard provider failure and silently falls back; no bounded re-prompt with the validation error. Hermes (`_invalid_json_retries`/`_invalid_tool_retries`) and OpenClaw (retry instructions) both do this. | 1, 7 | **P0** *(shipped — see §1)* | backend, tests, docs |
| 2 | **Structured error/liveness classifier + bounded transient retry** — Relux retry is a fresh run with no error taxonomy and no backoff; Paperclip classifies (`run-liveness.ts`) and retries transient upstream failures on a bounded `[2m,10m,30m,2h]` schedule. | 7 | **P1** *(shipped — see §14)* | backend, frontend, docs, tests |
| 3 | **Governed budgets (soft/hard, auto-pause)** — Paperclip enforces per-company/agent/project spend with warn + hard-stop + cancel-work. Relux records run `cost`/`usage` but enforces nothing. | 5 | P1 | backend, frontend, docs, tests |
| 4 | **Scoped permission grants (subtree / project)** — Relux permissions are exact-string match only; Paperclip has fine-grained grants scoped to manager-subtrees/projects. *(minimal plugin-scope `tool:<plugin>:*` SHIPPED — see §17; the `reports_to` org-lattice + acyclic-graph model SHIPPED — see §18; the manager-subtree SCOPED grant + one real enforcement path SHIPPED — see §19; the first **per-agent identity / access token** that lets a manager drive its own grant with no operator in the loop SHIPPED — see §20; a **second token-authenticated subtree action, `assign_task`,** SHIPPED — see §21; broader subtree actions / project / namespace scopes + agent-driven enrollment still missing.)* | 5 | P1 | backend, frontend, docs, tests |
| 5 | **Memory compaction / cross-session recall** — Relux kept a bounded 12-turn ring with no summarization; Hermes/OpenClaw compact + summarize + (Hermes) FTS5 cross-session search. *(in-session compaction beyond the ring SHIPPED — see §16; cross-session FTS recall still missing.)* | 6 | P1/P2 | backend, tests |
| 6 | **`execute_code` (RPC-from-script deterministic glue)** — the cheapest multi-step primitive; routes back through the same tool gate. Big, but high-leverage. | 2, 4 | P1 | backend, tests, docs |
| 7 | **Goal/issue hierarchy + monitor/recovery** — Relux orchestration is a flat ≤6-step DAG; Paperclip has Goal→Project→Issue→Run with monitor scheduling + stranded-issue recovery. | 4 | P2 | backend, frontend, docs, tests |
| 8 | **Session identity / handoff + safe resume** — Relux threw away the provider session id, so a run had no handoff record and could only be re-run cold; OpenClaw captures a per-provider CLI session binding and resumes it (`resumeSessionId` / `runCliWithSession`). | 3 | **P1** *(shipped — see §15)* | backend, frontend, docs, tests |

Slice #1 is the one chosen for this round (see §1, §13) because it is a true agentic-loop gap,
safe (adds no authority), bounded, feasible in one commit, and reuses existing validators.

---

## 1. Conversation loop & turn lifecycle

### Reference mechanism

- **Hermes** `agent/conversation_loop.py` `run_conversation(...)` (3,980 lines): a bounded ReAct
  loop. One assistant message carries BOTH `content` and `tool_calls`; the loop validates the
  chosen tool against a name allowlist, dispatches, injects the bounded result, and re-calls — the
  model "gives its final answer when it stops requesting tools." Loop guarded by `max_iterations`
  + an `IterationBudget`. **17 distinct API-error retry paths**; **7-path empty-response recovery**
  (partial-stream → prior-housekeeping → post-tool nudge → thinking-prefill → empty-retry →
  fallback-provider → give-up). `finish_reason="length"` → up to 3 continuation retries. Compression
  check runs AFTER tool execution. Fuzzy tool-name repair BEFORE erroring; `_invalid_tool_retries`
  / `_invalid_json_retries` counters inject explicit recovery messages and retry up to 3×.
- **OpenClaw** `src/agents/pi-embedded-runner/run.ts` (`runEmbeddedPiAgent`, ~L300+): layered loop
  (`agent-command.ts` → `command/attempt-execution.ts` → `pi-embedded-runner/run.ts`) with
  `MAX_RUN_LOOP_ITERATIONS`, auth-profile rotation, compaction-retry, idle-timeout breaker, and
  **retry instructions** (`resolvePlanningOnlyRetryInstruction`, `reasoningOnlyRetryAttempts`,
  `emptyResponseRetryAttempts`, `COMPACTION_CONTINUATION_RETRY_INSTRUCTION`) — the loop re-prompts
  with a corrective instruction rather than aborting.
- **Paperclip**: no in-process model loop; the loop is the heartbeat run (see §7).

### Relux mapping — **partial**

- `crates/relux-kernel/src/prime_decision.rs` — the **unified decision envelope**
  (`PrimeBrainDecision`, `parse_decision`) + the **bounded observe-then-act loop** (`DecisionLoop`,
  `DecisionStep`, `MAX_DECISION_ROUNDS = 3`, `run_decision_loop`). Each round the brain either
  requests read-only context tools (observe) or commits (act/answer); the kernel runs only the
  read-only tools between rounds and re-calls grounded in the results. Stop-on-repeat + round cap.
- `crates/relux-kernel/src/server.rs` `decide_prime_with_observation` (~L3198), `decide_prime_via_cli`
  (~L3166), `parse_cli_decision` (~L3252); `crates/relux-kernel/src/ai.rs`
  `decide_prime_via_openrouter` (~L867).
- The brain mirrors Hermes "one response carries everything" + "answer when it stops requesting
  tools." **But it acts ONCE at the end** (no act→observe-result→act loop — a deliberate safety
  choice) and, until this slice, **had no self-correction**: a malformed reply was
  `parse_decision(&text).ok()` → `None`, indistinguishable from a provider failure, → `Stop` → fall
  back to the deterministic rail.

### Priority & slices

- **P0 — bounded self-correction re-prompt (SHIPPED THIS ROUND).** Distinguish a *malformed but
  correctable* reply from a hard provider failure and re-ask the brain ONCE (bounded by
  `MAX_DECISION_CORRECTIONS`) with the exact `parse_decision` error injected, before falling back.
  Reuses `parse_decision`'s own `Err(String)` as the correction message (no new validator); adds no
  authority (a corrected decision still flows through the unchanged gate); worst case is byte-for-byte
  today. *(backend, tests, docs — see §13.)*
- **P2 — `finish_reason="length"`/empty-reply continuation** for the brain transports (Hermes's
  continuation retries), if real providers truncate the JSON envelope in practice.
- **P2 — second governed action in one turn** (act→observe-its-result→act). High value but **changes
  the safety model** (two mutations per turn); defer until a design doc covers re-gating the second
  action. *(backend, docs, tests.)*

---

## 2. Tool system (descriptors, validation, execution, no-leak envelopes)

### Reference mechanism

- **Hermes** `tools/registry.py` (`ToolRegistry`, `ToolEntry`: name/toolset/schema/handler/`check_fn`/
  `requires_env`/`max_result_size_chars`), `toolsets.py` (`resolve_toolset` recursive flatten +
  cycle detection), `model_tools.py` `handle_function_call` (~L802): `coerce_tool_args` (string→native,
  bare scalar→list), pre/post-tool plugin hooks, `_sanitize_tool_error` (strips role tags / CDATA /
  fences, caps at 2000 chars), progressive `tool_search`/`tool_describe`/`tool_call` bridge when tool
  defs exceed ~10% of the window. Read-vs-write hint via `_READ_SEARCH_TOOLS`.
- **OpenClaw** `src/tools/types.ts` (`ToolDescriptor`: name/inputSchema/owner/executor/availability),
  `src/tools/availability.ts` (`evaluateToolAvailability` — auth/config/env/plugin-enabled/context
  signals + `allOf`/`anyOf`), `src/tools/planner.ts` (`buildToolPlan` → visible/hidden split,
  `ToolPlanContractError` on dup/missing-executor), `src/tools/execution.ts` (`formatToolExecutorRef`:
  `core:`/`plugin:`/`channel:`/`mcp:`). `update-plan-tool.ts` `readPlanSteps` — per-entry compositional
  validation + status allowlist (the pattern Relux's `parse_decision` already mirrors).
- **Paperclip** `server/src/adapters/process/execute.ts` — adapter result is `{exitCode, signal,
  timedOut, resultJson:{stdout,stderr}}`; `cli/src/adapters/registry.ts` maps adapter type → stream
  formatter; redaction via `server/src/redaction.ts` (`sanitizeRecord`, `SECRET_FIELD_NAME_PATTERN`,
  secret-ref preservation).

### Relux mapping — **implemented (core)**

- `crates/relux-core/src/tool.rs` — `ToolDescriptor`, `ToolExecutability`
  (Ready/RuntimeNotConfigured/RuntimeDisabled/NotImplemented/MissingPermission/NeedsApproval — honest,
  never fabricated), `approval_blocks_direct_invocation`.
- `crates/relux-kernel/src/builtin.rs` — `BUILTIN_TOOLS` (only `echo.say`, `status.summary`).
- `crates/relux-kernel/src/prime_tools.rs` — read-only allowlist + `validate_tool_request` +
  `execute_requested_reads` (`MAX_TOOL_ROUNDS`, `MAX_RESULT_CHARS`).
- `crates/relux-kernel/src/prime_write_tools.rs` — `classify_write_tool` (fail-closed name allowlist),
  `parse_write_tool_request` → reuses per-action slot validators.
- `crates/relux-core/src/adapter_result.rs` `parse_adapter_result` (no-leak envelope parse),
  `crates/relux-core/src/redact.rs` `redact_secrets`.

### Priority & slices

- **P1 — `execute_code` (RPC-from-script)**: model writes one script that calls read/write tools over
  a local loopback RPC routed back through the SAME `prime_write_tools` gate + approval; only stdout
  returns. Maps to Hermes `tools/code_execution_tool.py` (`SANDBOX_ALLOWED_TOOLS`, `_scrub_child_env`,
  budget refund). High leverage, but a real subsystem. *(backend, tests, docs.)*
- **P2 — arg coercion / fuzzy tool-name repair before fail** (Hermes `coerce_tool_args` /
  `fuzzy_match.py`): when a write-tool name is a near-miss (`task_create` vs `task.create`), repair
  before dropping. Composes with the §1 self-correction slice. *(backend, tests.)*
- **P2 — progressive tool disclosure** once the write/read tool catalog grows past a threshold.

---

## 3. Agent / subagent / session model

### Reference mechanism

- **Hermes** `tools/delegate_tool.py`: synchronous, **non-durable** subagents (cancelled on
  interrupt, work discarded). `DELEGATE_BLOCKED_TOOLS` (no recursion, no clarify, no memory),
  `MAX_DEPTH=1` (cap 3), `_DEFAULT_MAX_CONCURRENT_CHILDREN=3`, ThreadPool with non-interactive
  approval callback (`_subagent_auto_deny` default), `_active_subagents` registry +
  `interrupt_subagent`, `_extract_output_tail`.
- **OpenClaw** `src/agents/acp-spawn.ts`: `spawnAcp` (mode "run"|"session", resume by
  `resumeSessionId`), `getSubagentDepthFromSessionStore` (`DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH`),
  `countActiveRunsForSession` (`DEFAULT_SUBAGENT_MAX_CHILDREN_PER_AGENT`), thread bindings, sandbox
  "inherit"/"require", workspace inheritance, session resolution by id/key/agentId.
- **Paperclip** `packages/db/src/schema/agents.ts`: durable agents with `reportsTo` org tree
  (indexed `(companyId, reportsTo)`), roles, capabilities, per-agent budget, `lastHeartbeatAt`;
  `authorization.ts` `agentIsInSubtree` (50-depth walk). The **durable, outlive-the-turn** model.

### Relux mapping — **partial** *(session identity / handoff + Claude resume now implemented — see §15; the `reports_to` org-lattice model now implemented — see §18)*

- `crates/relux-core/src/agent.rs` — `Agent` (id/name/description/adapter/persona/skills/status/
  permissions/namespace), `AgentStatus`. `crates/relux-kernel/src/agent_config.rs`,
  `agent_presets.rs` — manual crew config + role presets.
- Assignment/target resolution: `crates/relux-core/src/orchestration.rs` `resolve_assignee`
  (exact→prefix→substring against the live roster); skill-aware matching.
- **Session identity / handoff / resume**: `crates/relux-core/src/run_session.rs` — `RunSession`
  (bounded, redacted `adapter_session_id` + `source` + per-kind `resume_supported`),
  `sanitize_session_id` (argv-safe, leading-dash-rejected, length-bounded), `plan_resume` →
  `ResumeDisposition`. The Claude `--output-format json` envelope's `session_id` is lifted by
  `parse_adapter_result`, stamped on the `Run` (`session`), and a real `run.resume` continues that
  session through the governed gate (Claude `-p --resume <id>`, `build_resume_adapter_args`,
  threaded in `prepare_cli_run` only when `resumed_from` is set); Codex/Command honestly refuse.
  Maps OpenClaw `getCliSessionBinding(...).sessionId` + `runCliWithSession(nextCliSessionId, ...)`.
- **Durable agents** exist (they outlive the turn and run via the orchestration batch). The
  **`reports_to` org-lattice / chain-of-command model is now implemented** (see §18): an optional Lead
  pointer on every operative, validated acyclic at the config boundary, with pure
  `relux_core::hierarchy` walks (`chain_of_command`, `is_in_subtree`, `would_create_cycle`). **Still
  missing**: subagent spawn-depth/children caps (orchestration has step/concurrency caps instead),
  resume of a Codex session / mid-run partial resume (no provider session id is captured on the Codex
  plain-text path), and the **subtree-SCOPED permission enforcement** the helper is built for (it pairs
  with §5 — no grant reads the subtree yet, by design this round).

### Priority & slices

- **P2 — `reports_to` chain-of-command (SHIPPED THIS ROUND, §18).** The org-lattice MODEL — an
  optional Lead pointer, acyclic-validated on create/edit, with pure subtree/chain helpers — now
  exists. The remaining half is **manager-subtree authority**: a permission grant that reads
  `is_in_subtree` (Paperclip `scopeAllows` + `agentIsInSubtree`); that enforcement is deliberately NOT
  wired this round (the model ships first, the scope later). *(backend, tests, docs.)*
- **P1 — session identity / handoff + safe Claude resume (SHIPPED THIS ROUND, §15).** Capture +
  persist the adapter session id (bounded/redacted), expose it on the run detail (copyable, honest
  resume-supported label), and a real `run.resume` for the Claude CLI through the existing governed
  adapter gate; everything else refuses honestly (`ResumeNotSupported`). Maps OpenClaw
  `resumeSessionId` / `runCliWithSession`. Re-run/fresh retry (§7) stays distinct.

---

## 4. Planning / orchestration

### Reference mechanism

- **Paperclip** (the deep one): `packages/db/src/schema/{goals,projects,issues}.ts` —
  Goal(level: task/epic/strategic, `parentId`) → Project(`goalId`, `leadAgentId`, env) →
  Issue(`parentId` sub-issues, dual assignee, status FSM backlog→…→done, `checkoutRunId`/`executionRunId`
  lease, `originKind`/`originFingerprint` dedup, **monitor** fields `monitorNextCheckAt`/`monitorAttemptCount`).
  `issue-continuation-summary.ts` (auto-doc, 8k cap, mode inference). Child-issue creation capped at 25.
- **OpenClaw** `src/agents/tools/update-plan-tool.ts` `readPlanSteps` — per-step validation + status
  allowlist (a plan is steps with statuses, validated compositionally).
- **Hermes**: no goal hierarchy; flat kanban routed by assignee string.

### Relux mapping — **partial**

- `crates/relux-core/src/orchestration.rs` — `OrchestrationRole`, `PlannedStep` (role/agent/`depends_on`
  DAG), `OrchestrationPlan` (`is_multi_agent` = ≥2 steps), `plan_orchestration` (pure: split goal →
  classify role → resolve agent → infer DAG, `MAX_STEPS=6`), `Orchestration`/`OrchestrationStep`
  (outcome FSM Pending/Completed/Failed/Blocked), `OrchestrationBatchResult`.
- `crates/relux-kernel/src/state.rs` `run_orchestration` (rounds, parallel within concurrency cap,
  dependency-gated), `prime_orchestration_slots.rs` (governed `orchestration.create`/`.start`).
- A real, governed, deterministic multi-agent planner+executor — but **flat** (no Goal→Project→Issue
  hierarchy, no goal ancestry, no monitor/recovery scheduling, no sub-issue tree).

### Priority & slices

- **P2 — goal hierarchy** (Goal→Project→Orchestration) so an orchestration can hang off a durable
  goal with ancestry. *(backend, frontend, docs, tests.)*
- **P2 — monitor/recovery scheduling** for a running orchestration (`monitorNextCheckAt`-style
  re-check + stranded-step recovery). Pairs with §7. *(backend, tests.)*

---

## 5. Approval / permission / governance

### Reference mechanism

- **Paperclip** (the deep one): `approvals` table (type e.g. `hire_agent`, status
  pending/revision_requested/approved/rejected, `payload`), `services/approvals.ts` (approve →
  side-effect e.g. activate agent). `principal_permission_grants` — fine-grained
  `(principal, permissionKey, scope)` with scope = projectIds/agentIds/**managerAgentId-subtree**;
  `authorization.ts` `scopeAllows` + `agentIsInSubtree`. **Budgets** `services/budgets.ts` —
  company/agent/project scope, monthly+lifetime windows, `warnPercent` soft + hard `amount`,
  `cancelWorkForScope()` on breach, `budgetIncidents`. Board oversight.
- **OpenClaw** `src/acp/permission-relay.ts` — exec approval as a relay: `buildAcpPermissionRequest`
  → options `allow-once` (one-shot) / `allow-always` (persistent) / `deny`;
  `resolveGatewayDecisionFromPermissionOutcome`.
- **Hermes** `tools/approval.py` — `_YOLO_MODE_FROZEN` (snapshot at import, injection-proof),
  dangerous-pattern + sensitive-write-target detection, hardline blocklist for uncontainerized
  backends, plugin observability hooks.

### Relux mapping — **partial**

- `crates/relux-core/src/approval.rs` (`Approval`, `ApprovalStatus`), `permission.rs` (`Permission`
  prefix-validated, `RiskLevel`, `ApprovalRequirement` Never/Required/RequiredWhenRisk).
- `crates/relux-kernel/src/state.rs` — `decide` → risky intents become `Propose` behind one-shot human
  approval; safe → `Act`. Per-tool-call binding approval (executes once) vs generic approval
  (executes nothing) — two distinct surfaces. Permission check at `start_run` (agent must hold all
  `required_permissions`).
- **One-shot approval + fail-closed gate + per-tool approval are implemented.** **Scoped grants —
  minimal plugin scope SHIPPED THIS ROUND (see §17).** `relux_core::Permission` now also accepts the
  single scoped wildcard `tool:<plugin-id>:*` (strict grammar; every broader/partial/non-tool glob and
  any path-like string is rejected fail-closed), and enforcement compares grant-vs-required through
  `Permission::authorizes` (exact OR same-plugin scope) at the one `agent_holds_permission` chokepoint
  + the `start_run` task check. Grant/revoke bookkeeping stays exact-match, so a scope is one explicit,
  individually-revocable row that never pattern-expands. The **`reports_to` org-lattice an agent-subtree
  scope needs now exists** (see §18 — `relux_core::hierarchy::is_in_subtree`), and **one real
  manager-subtree grant now consults it** (SHIPPED THIS ROUND — see §19): `relux_core` accepts the
  strict `agent:<manager-id>:subtree:<action>` grant, `manager_subtree_authorizes` is the pure
  grammar+`is_in_subtree` matcher, and the kernel `manager_grant_permission_to_subordinate` path lets a
  *live* manager grant a permission to an operative inside its OWN Branch (and only there). **Still
  missing**: budgets/spend enforcement (runs record `cost`/`usage` but nothing enforces a ceiling), the
  *broader* scope vocabulary (project / namespace scopes; more subtree *actions* — `grant_permission`
  SHIPPED §19/§20, `assign_task` SHIPPED §21, others e.g. `revoke` still open).
  The **agent-actor surface that invokes the manager-grant path now exists** (SHIPPED — see §20: a
  per-agent access token authenticates the manager directly on `POST /v1/relux/agents/me/manager-grant`,
  no operator in the loop); the operator-assisted HTTP/UI path (§19) remains as the operator-console
  affordance. Still open: persistent `allow-always` grants, agent-driven token enrollment, Board-style
  multi-party oversight.

### Priority & slices

- **P1 — governed budgets** (`budget.rs` core type + kernel enforcement): per-namespace/agent soft
  warn + hard stop that pauses new runs and surfaces a Doctor/approval signal. Maps to Paperclip
  `budgets.ts`. *(backend, frontend, docs, tests.)*
- **P1 — scoped permission grants (minimal plugin scope SHIPPED in §17; `reports_to` lattice SHIPPED
  in §18).** `Permission` gained a strictly-validated `tool:<plugin-id>:*` scope + an `authorizes`
  enforcement comparison (§17), and the org lattice the larger half needs — `reports_to` +
  `is_in_subtree` — now exists (§18). The remaining open piece is the **agent-subtree grant
  enforcement** itself: a permission scoped to a manager's Branch that authorization actually consults
  (Paperclip `scopeAllows` + `agentIsInSubtree`). The helper is built and tested; wiring it into a
  grant is the next slice. *(backend, tests, docs.)*
- **P2 — persistent `allow-always` approval** (OpenClaw one-shot vs persistent): an approval that
  records a standing grant so the same safe action isn't re-prompted. Must stay revocable. *(backend,
  frontend, tests.)*

---

## 6. Memory / context

### Reference mechanism

- **Hermes** (richest): `MemoryStore` (char-bounded MEMORY.md/USER.md, frozen system-prompt snapshot
  for prefix-cache stability, security scan + injection regex on write, atomic `os.replace`),
  `context_compressor.py` (compress at 50% window: 3-pass tool-result pruning → tail-boundary
  algorithm protecting last-user-message → 12-section LLM summary with redaction → anti-thrash
  counter), SQLite + dual FTS5 (`unicode61`+`trigram`) cross-session search.
- **OpenClaw** `src/context-engine/types.ts` — `assemble`/`compact`/`ingest`/`maintain` contract;
  `CompactResult` (summary + `firstKeptEntryId` + token deltas + session rotation);
  `rewriteTranscriptEntries` for safe edits; `promptAuthority` overflow flag.
- **Paperclip** `services/activity-log.ts` (sanitized JSONB), `issue-continuation-summary.ts`
  (8k-cap auto-summary doc, mode inference) — durable, not in-context compaction.

### Relux mapping — **partial**

- `crates/relux-kernel/src/prime_history.rs` — bounded ring: `MAX_HISTORY_TURNS=12`,
  `MAX_HISTORY_CONVERSATIONS=32`, `MAX_CONTEXT_CHARS=2000`, `sanitize_text` (redaction + control-char
  strip + clamp); tool reads stored as name+1-line summary, never bodies. Rendered as labelled
  BACKGROUND (the Hermes `<memory-context>` "reference, not instruction" shape).
- `crates/relux-kernel/src/prime_clarify_memory.rs` — single TTL-bounded pending clarification per
  conversation key (`resolve_pending`: Cancelled/Expired/FreshRequest/Continue).
- `crates/relux-core/src/redact.rs` — secret redaction applied to transcripts + history.
- **Compaction beyond the ring (SHIPPED THIS ROUND — see §16).** When a turn ages OUT of the
  12-turn ring it is folded into a rolling, bounded, secret-redacted, **deterministic**
  per-conversation `relux_core::ConversationSummary` (ids the turn created + a chat-turn count +
  the opening message), rendered at the TOP of the same BACKGROUND block before the recent turns
  (`crates/relux-kernel/src/prime_history.rs` `fold_evicted_turn` / `render_context_with_summary`;
  `KernelState.conversation_summaries`). No provider call; advisory only; `clear_conversation`
  drops it too.
- **Implemented**: bounded, redacted, advisory history + clarify memory + in-session compaction.
  **Missing**: cross-session recall (no FTS/search), and a brain-generated (vs deterministic)
  summary.

### Priority & slices

- **P1/P2 — compaction/summarization beyond the ring (SHIPPED THIS ROUND, §16).** Deterministic
  fold of evicted turns into a bounded redacted summary (OpenClaw `CompactResult` summary +
  kept-entries / Hermes `context_compressor` head-protect + bounded digest / Paperclip
  `issue-continuation-summary` deterministic char-bounded extraction).
- **P2 — brain-generated summary** as a strictly-additive, strictly-validated, off-lock overlay
  over the deterministic one (Hermes 12-section LLM summary), with a deterministic fallback and no
  unbounded calls. *(backend, tests.)*
- **P2 — cross-session recall** (Hermes FTS5 "bookends"): deterministic, LLM-free lookup of a prior
  resolution. *(backend, tests.)*

---

## 7. Error handling / recovery

### Reference mechanism

- **Hermes** `conversation_loop.py`: 17 API-error retry paths (`classify_api_error` →
  `FailoverReason`), jittered backoff capped at `Retry-After`, 7-path empty-response recovery,
  file-mutation verifier footer (can't claim an edit that failed). Tool layer: `_sanitize_tool_error`,
  `coerce_tool_args`, fuzzy repair.
- **Paperclip** `server/src/services/run-liveness.ts` (`RunLivenessState`, evidence-based regex
  classification: planning-only/blocked/approval-required/manager-review), `services/heartbeat.ts`
  transient retry `[2m,10m,30m,2h]` + 25% jitter (max 4) for `*_transient_upstream`, max-turn
  continuation (2×), `services/recovery/service.ts` (watchdog decisions, output-silence thresholds
  60m/4h, stranded-issue + stale-run recovery).
- **OpenClaw** `attempt-execution.ts` `FailoverError`+`classifyEmbeddedPiRunResultForModelFallback`
  (session_expired/model_not_found/auth/rate_limit/overflow/timeout), profile rotation, idle-timeout
  breaker, retry instructions (§1).

### Relux mapping — **partial** *(error classifier + bounded transient retry now implemented — see §14)*

- `crates/relux-core/src/adapter_result.rs` — honest parse, plain-text fallback, never fabricates
  success/failure. `KernelError` taxonomy (UnknownTask/Agent, PermissionDenied, …).
- `crates/relux-core/src/run_failure.rs` — **the structured classifier**: `RunFailureClass`
  (`transient_provider`/`auth_required`/`adapter_missing`/`permission_denied`/`invalid_prompt`/
  `timeout`/`cancelled`/`output_validation`/`unknown`), priority-ordered `classify_failure`,
  `retryable()`/`needs_operator_action()`/`remediation()`, the bounded `RETRY_BACKOFF_SECS =
  [2m,10m,30m,2h]` schedule, and `RunRetryState::plan`. Now stamped on every failed `Run`
  (`failure_class` + `retry`).
- `crates/relux-kernel/src/state.rs` — `fail_run_classified` stamps the class + bounded-retry state;
  `transient_retry_ready(now)` is the read-only retry-ready projection; `one_autonomy_tick` re-attempts
  eligible transients through the unchanged governed `retry_run` path (which still re-checks runtime/
  PATH/permission and stamps `retried_from`, so the backoff grows + exhausts). No background scheduler:
  eligibility is a real wall-clock not-before checked only on a manual retry or an operator/cron tick.
- Brain loop: `DecisionLoop` distinguishes a malformed-but-correctable reply from a hard provider
  failure and self-corrects once (§1/§13).
- **Still missing**: output-silence/stranded recovery for long orchestration runs, circuit-breaking,
  partial-run resume.

### Priority & slices

- **P0 — self-correction on malformed decisions (SHIPPED, §1/§13).**
- **P1 — error classifier + bounded transient retry (SHIPPED THIS ROUND, §14).** A `RunFailureClass`
  (transient_provider / timeout / auth / adapter / permission / invalid / cancelled / output_validation /
  unknown) + a bounded `[2m,10m,30m,2h]` backoff retry for the two safe transient classes only, behind
  the existing governed run path. Maps to Paperclip `run-liveness.ts` + the `[2m,10m,30m,2h]` schedule
  and Hermes `error_classifier.py`.
- **P2 — output-silence/stranded recovery** for long orchestration runs. Pairs with §4 monitor. *(backend, tests.)*

---

## 8. CLI / process / runtime adapters

### Reference mechanism

- **Hermes** `tools/environments/base.py` — one `BaseEnvironment.execute()` over six backends
  (local/docker/ssh/singularity/modal/daytona); `_ThreadedProcessHandle` wraps blocking SDKs;
  `touch_activity_if_due` activity→heartbeat bridge; `sync_back` on teardown; `ansi_strip`. Serverless
  hibernate-to-$0 (Modal snapshot / Daytona stop-resume keyed by task_id).
- **OpenClaw** `src/process/exec.ts` — `runExec`/`execFile` with timeout+maxBuffer,
  `shouldSpawnWithShell()` hardcoded false (no argv injection), Windows `.cmd`/npm shim resolution
  (CVE-2024-27980), `WINDOWS_UNSAFE_CMD_CHARS_RE`, no-output-timeout, `AbortSignal`.
- **Paperclip** `server/src/adapters/process/execute.ts` — `runChildProcess` with `timeoutSec`/`graceSec`
  (15s), streaming `onLog`, env via `buildPaperclipEnv` + `ensurePathInEnv`, invocation-meta logging,
  `cli/src/adapters/registry.ts` (11 adapter types).

### Relux mapping — **implemented (core)**

- `crates/relux-core/src/adapter.rs` (`AdapterKind`: LocalPrime/ClaudeCli/CodexCli/Command),
  `crates/relux-kernel/src/adapter.rs` (`AdapterCommandSpec`: program/args/stdin/working_dir/timeout/
  max_output_bytes; `AdapterRunOutcome` with real wall-clock duration + truncation flags).
- **argv-only (no shell injection), non-bypass** (Claude `--permission-mode default`, never
  `--dangerously-skip-permissions`), bounded timeout + output cap, secret-redacted output, read-only
  PATH probe (`find_on_path`). Two CLI-stdout shaping seams both go through `parse_adapter_result`.
- **Missing (deliberate/deferred)**: serverless/sandboxed backends, streaming token output to the UI,
  mid-run cancellation.

### Priority & slices

- **P2 — streaming run-log tails to the UI** (Paperclip `onLog`): see §10. *(backend, frontend, tests.)*
- **P2 — mid-run cancellation** (`AbortSignal`-style) for a long adapter spawn. *(backend, tests.)*
- **Deferred — serverless backends** (Hermes Modal/Daytona): the "execution workspaces" phase.

---

## 9. Plugin / tool install & configuration

### Reference mechanism

- **OpenClaw** `src/plugins/manifest.ts` + `bundle-manifest.ts` — JSON5 manifests
  (`.codex-plugin/`, `.claude-plugin/`, `openclaw.plugin.json`), `MAX_PLUGIN_MANIFEST_BYTES=256k`,
  activation triggers (`onStartup`/`onProviders`/…), boundary-checked reads, `allowMissing` →
  empty `{}`.
- **Paperclip** `server/src/services/plugin-manifest-validator.ts` (Zod, version-gated,
  never-throws safe-parse), `plugin-registry.ts` (CRUD, soft-delete reinstall reuses the row),
  `adapters/plugin-loader.ts` (lazy UI-parser extraction).
- **Hermes** `hermes_cli/plugins.py` (`plugin.yaml`+`register(ctx)`, kinds standalone/backend/…,
  enable/disable allowlists), `tools/mcp_tool.py` (stdio/HTTP/SSE MCP servers + per-server timeouts +
  sampling), `mcp_oauth_manager.py` (disk-watch refresh, 401 dedup).

### Relux mapping — **implemented (core)**

- `crates/relux-core/src/plugin.rs` (`PluginId`, `PluginKind`, `TrustLevel`, `InstalledPlugin`,
  `PluginSourceKind` Bundled/LocalDir/Zip/Github), `crates/relux-kernel/src/plugin_install.rs`
  (install-from-dir/github/zip, manifestless install), `plugin_tool_config.rs` (per-tool runtime
  config), `crates/relux-core/src/runtime.rs` (`ToolRuntimeConfig` HTTP loopback-only — rejects https /
  non-loopback / creds / traversal).
- Honest discovery (installed-but-unimplemented → `NotImplemented`), bundled-protected, no remote code
  execution by design.
- **Missing (deliberate/deferred)**: MCP server support, plugin activation triggers.

### Priority & slices

- **P2 — MCP tool support** (Hermes `mcp_tool.py`): the standard external-tool protocol; large.
  *(backend, tests, docs.)*
- **P2 — install-time manifest validation surfaced in Doctor/UI** (Paperclip Zod safe-parse style).

---

## 10. UI / product ergonomics

### Reference mechanism

- **Paperclip** `ui/src/pages/*` (28 pages) — issues-as-conversations (threaded comments + doc
  annotations), board, **org-chart SVG/PNG** (`routes/org-chart-svg.ts`), inbox, approvals
  (`ApprovalDetail.tsx`), first-run board-claim + CLI-auth onboarding, workflow-stage visualization.
- **OpenClaw**/**open-webui** — streaming chat, tool-call visibility, readiness signals.

### Relux mapping — **implemented (core)**

- `apps/dashboard/` (declarative `BrowserRouter`, `useAsync` not route loaders) — chat surface with
  provenance chips (`🔎 used:`, `🛠 requested tool:`, `🧠 brain-worded`, `⏳ waiting for:`), crew
  config + role presets, approvals view, runs/tasks views, plugins tab, **Doctor panel** (read-only
  health). Dashboard bundle is the git-tracked build output in
  `crates/relix-web-bridge/dashboard-dist`.
- **Partial/missing**: live streaming run-log tails (two run-transcript surfaces exist but tailing is
  limited), an org chart, issue-as-conversation threading.

### Priority & slices

- **P2 — live run-log tail in the runs view** (pairs with §8 streaming). *(frontend, backend, tests.)*
- **P2 — crew org-chart view** once `reports_to` (§3) lands. *(frontend.)*

---

## 11. Security / safety

### Reference mechanism

- **Hermes**: memory-context fencing (recalled memory wrapped in a trusted fence, forged fences
  stripped, streaming scrubber), `redact_sensitive_text` before every summarizer call, `_scrub_child_env`,
  YOLO frozen at import.
- **Paperclip** `server/src/{redaction,log-redaction}.ts` (secret-field + CLI-flag patterns, username/
  home masking, secret-ref preservation), `middleware/auth.ts` + `agent-auth-jwt.ts` (HMAC-SHA256,
  timing-safe verify), company-boundary multi-tenant isolation, `board-claim.ts` first-admin claim.
- **OpenClaw** `src/secrets/audit.ts` (plaintext/ref-unresolved findings), no-shell exec, manifest
  boundary reads.

### Relux mapping — **implemented**

- `crates/relux-core/src/redact.rs` (`redact_secrets`: sk-ant-/sk-/ghp_/gho_/ghu_/ghs_/ghr_/xoxb-/…/
  AKIA/AIza + key=value, structure-preserving), `crates/relux-kernel/src/auth.rs` (Argon2id, session
  cookie `relux_session`, TTL + absolute-max, middleware gating, `RELUX_AUTH_DISABLED` dev escape with
  loud warning).
- Loopback-only HTTP bind, argv-only CLI, no remote plugin code. Fail-closed gates: `reconcile_intent`
  (guarded chat never becomes work), id validation against the live snapshot, terminal-state guard,
  read-only-tool allowlist, write-tool allowlist, `start_run` permission check.
- Strong for Relux's local-operator threat model.

### Priority & slices

- **P2 — memory-context fencing** for the rendered history block (Hermes): treat recalled history as
  untrusted-by-default and strip any forged control fences. The §1 self-correction reply path is also
  a place to ensure injected error text can't carry instructions (it is kernel-authored, so safe).
  *(backend, tests.)*

---

## 12. Release / ops / devex

### Reference mechanism

- **Hermes** `setup-hermes.sh`, nix flake, `cli.py`, docker; rich docs. **OpenClaw** `openclaw.mjs`,
  pnpm workspace, tsdown, vitest. **Paperclip** pnpm workspace, `startup-banner.ts`, vitest.

### Relux mapping — **implemented**

- `Start-Relux.ps1` + `start-relux.sh` (cross-platform source launchers), `install.ps1`/`install.sh`,
  `crates/relux-kernel/src/doctor.rs` (read-only `DoctorReport`/`DoctorCheck` with severity +
  remediation, no heavy work), embedded `#[cfg(test)]` unit tests + kernel integration tests.
- Releases are cut **manually via `gh` + `relux-v0.1.x` tags** (the `v*` Actions workflow never matches
  `relux-` tags; `dist/` is gitignored). GitHub Actions stays disabled.

### Priority & slices

- **P2 — expand Doctor checks** as new subsystems (budgets, error classifier) land, each a pure
  projection + a check entry. *(backend, tests.)*

---

## 13. Implemented this round — the self-correction slice (§1 P0)

See the matching entries in [`reference-driven-development.md`](reference-driven-development.md) and
[`prime-processing-audit.md`](prime-processing-audit.md) for the reference read + the applied-change
record. In brief:

- **What**: the bounded observe-then-act `DecisionLoop` now distinguishes a *malformed but
  correctable* brain reply (`DecisionOutcome::Malformed(err)`) from a hard provider failure
  (`DecisionOutcome::ProviderError`). On a malformed reply it re-asks the brain ONCE
  (`MAX_DECISION_CORRECTIONS`) with the exact `parse_decision` validation error injected into the
  prompt (`build_decision_prompt`'s new `correction` block), before falling back to the deterministic
  rail.
- **Why it is safe**: the correction only asks the brain to fix its OUTPUT FORMAT; it grants no new
  authority. A corrected decision still flows through the unchanged fail-closed gate
  (`reconcile_intent` → slot validators → `decide` → `prime_execute` / approval). Total brain calls
  stay bounded (`MAX_DECISION_ROUNDS + MAX_DECISION_CORRECTIONS`). A provider failure does NOT retry
  (re-calling a broken provider wastes calls and risks a spin). Worst case is byte-for-byte today's
  behavior (malformed → bounded correction fails → fall back). No wire/dashboard change.
- **Reuse, not duplication**: the correction message IS `parse_decision`'s own `Err(String)` — no new
  or weaker validator. The synchronous twin `run_decision_loop_with_correction` and the async driver
  share the SAME `DecisionLoop::step_outcome` stepper, so the control flow (round cap, correction cap,
  read-only execution, stop-on-progress) is pinned once.

---

## 14. Implemented this round — the error classifier + bounded transient retry slice (§7 P1)

See the matching "Reference read — structured run-failure classifier + bounded transient retry"
entry in [`reference-driven-development.md`](reference-driven-development.md) for the full reference
read + mapping. In brief:

- **What.** A structured `RunFailureClass` (`crates/relux-core/src/run_failure.rs`) classifies every
  failed run into one of nine classes — `transient_provider`, `auth_required`, `adapter_missing`,
  `permission_denied`, `invalid_prompt`, `timeout`, `cancelled`, `output_validation`, `unknown` —
  via a priority-ordered, pattern-driven `classify_failure` (mirroring Hermes `error_classifier.py`).
  Each class carries `retryable()`, `needs_operator_action()`, and a safe static `remediation()`. A
  failed `Run` now records `failure_class` and, for an auto-retryable class, a `RunRetryState` that
  schedules the next attempt on the bounded `[2m,10m,30m,2h]` backoff (Paperclip
  `heartbeat.ts` `BOUNDED_TRANSIENT_HEARTBEAT_RETRY_DELAYS_MS`), capped at four attempts.
- **Where it surfaces.** The class + retry state + a derived `failure_remediation` flow onto the run
  wire (`server.rs` `RunRecord`); the Work page shows a Failure-class chip, an honest Recovery line
  (scheduled / due / exhausted / needs-operator-action), and the remediation; a new Doctor
  `runs.recovery` row warns when failed runs need an operator and notes transient retries pending.
- **Retry without a faked scheduler.** There is NO background timer (the audit's explicit honesty
  constraint). `not_before_secs` is a real wall-clock instant; `transient_retry_ready(now)` is a
  read-only projection consumed either by the MANUAL `prime.retry_run` or by `one_autonomy_tick`,
  which re-attempts eligible transients through the UNCHANGED governed `retry_run` path (re-checking
  the enabled runtime, the binary on PATH, and the permission gate, and stamping `retried_from` so
  the backoff grows attempt-by-attempt and exhausts at the cap).
- **Why it is safe.** The classifier is a pure, deterministic projection that grants no authority. Only
  the two unambiguously-safe, upstream-caused classes (`transient_provider`, `timeout`) auto-retry;
  every other failure — including, stricter than Hermes, the `unknown` catch-all (a Relux run can
  mutate a workspace) — surfaces a remediation and waits for an operator. A retry never bypasses the
  adapter/approval gates. Surfaced strings are redacted + clamped (`safe_public_message`).

---

## 15. Implemented this round — durable session identity / handoff + safe Claude resume (§3 P1)

See the matching "Reference read — session identity / handoff / resume" entry in
[`reference-driven-development.md`](reference-driven-development.md) for the full reference read +
mapping. In brief:

- **Reference read (BINDING).** OpenClaw `src/agents/acp-spawn.ts` (`resumeSessionId`,
  `validateAcpResumeSessionOwnership`, `sessionEntryMatchesAcpResumeSessionId`) and
  `src/agents/command/attempt-execution.ts` (`getCliSessionBinding(sessionEntry, "claude-cli").sessionId`,
  `runCliWithSession(nextCliSessionId, activeCliSessionBinding)`,
  `claudeCliSessionTranscriptHasContent` → reset-on-missing, `FailoverReason::session_expired`):
  a per-provider CLI **session binding** is captured, then optionally **resumed** through the same
  spawn gate; an expired/empty session resets to fresh rather than being faked. Hermes
  `tools/delegate_tool.py` confirms the contrast (synchronous, **non-durable** subagents, no resume).
  Paperclip `packages/db/src/schema/agents.ts` confirms the durable-agent baseline (already present in
  Relux). Relux files read/mapped: `crates/relux-core/src/{run.rs,adapter.rs,adapter_result.rs}`,
  `crates/relux-kernel/src/{adapter.rs,state.rs,server.rs}`,
  `apps/dashboard/src/{api.ts,runview.ts,pages/Work.tsx}`.

- **What.** The Claude CLI `--output-format json` envelope carries a top-level `session_id`.
  `parse_adapter_result` lifts it (`AdapterResultSummary.session_id`);
  `relux_core::RunSession::from_envelope` sanitizes it (argv-safe charset, leading-dash rejected,
  `MAX_SESSION_ID_LEN`-bounded) and records a bounded, redacted `RunSession { adapter_session_id,
  source, resume_supported }` on the `Run` (`set_run_session`, both the success and error-envelope
  paths). `AdapterKind::resume_supported()` is the honest per-kind capability — **only** the Claude CLI
  qualifies. We store ONLY the session id + source + capability — never a raw envelope, token, or full log.

- **Resume (real, not faked).** `Run.resumed_from` is a distinct lineage field (separate from
  `retried_from`). `KernelState::resume_run` uses the pure `relux_core::plan_resume` decision: a
  terminal run carrying a `resume_supported` session is resumed through the SAME governed CLI gate
  (enabled runtime + PATH probe + permission check + bounded, non-bypass spawn), threading
  `--resume <session_id>` via `build_resume_adapter_args` (only when `resumed_from` is set, so resume
  never leaks onto a cold run); everything else returns `RunResumeNotSupported` with a specific reason
  (422 on the wire). An invalid/expired session simply fails honestly when the CLI rejects it. The new
  run is stamped `resumed_from`, audited (`run:resume`), and recorded on the transcript
  (`run_resumed_from`). Re-run/fresh retry (§7) stays a distinct action.

- **Where it surfaces.** The `session` + `resumed_from` fields flow onto the run wire (flattened
  `RunRecord`), plus a derived `resumable` flag. The Work page Run Detail shows a copyable Session id,
  an honest Handoff label (`sessionHandoffLabel` — "resume supported" vs "resume not supported here;
  kept for handoff/audit"), a "Resume of" lineage link, and a **Resume session** button (distinct from
  Retry) gated by `canResumeRun`. `POST /v1/relux/runs/:id/resume` backs it.

- **Why it is safe / honest.** Resume reuses the unchanged governed adapter path (argv-only,
  non-bypass, bounded, redacted); it grants no new authority. It is never represented as a process
  resume the adapter cannot do — Codex/Command/local-prime refuse with a clear reason. The capability
  flag, the UI label, and the action all read from the single `plan_resume` source of truth, so they
  cannot disagree.

- **Still missing (honest).** Codex-session resume and mid-run *partial* resume (the Codex `exec`
  plain-text path emits no session id we capture); no cross-session search over stored session ids
  (pairs with §6 cross-session recall).

---

## 16. Implemented this round — bounded conversation-memory compaction beyond the ring (§6 P1)

See the matching "Reference read — bounded conversation-memory compaction beyond the ring" entry in
[`reference-driven-development.md`](reference-driven-development.md) and the
[`prime-processing-audit.md`](prime-processing-audit.md) "Bounded conversation-memory compaction"
section for the full reference read + applied-change record. In brief:

- **Reference read (BINDING).** Hermes `agent/context_compressor.py` (head/tail-protected pruning +
  bounded redacted summary of the older middle, anti-thrash) and `agent/memory_manager.py`
  (`build_memory_context_block` background fence). OpenClaw
  `src/context-engine/types.ts` (`CompactResult.result = { summary, firstKeptEntryId, ... }` — a
  summary stands in for everything before the kept-entries boundary, prepended via
  `AssembleResult.systemPromptAddition`). Paperclip
  `server/src/services/issue-continuation-summary.ts` (deterministic, char-bounded
  (`ISSUE_CONTINUATION_SUMMARY_MAX_BODY_CHARS = 8_000`), `truncateText` honest `[truncated]`
  marker, salient-fact extraction without a model call). Relux files read/mapped:
  `crates/relux-kernel/src/{prime_history.rs,state.rs,store.rs}`,
  `crates/relux-core/src/{prime.rs,lib.rs}`.

- **What.** The recent ring (`MAX_HISTORY_TURNS = 12`) is unchanged, but `push_bounded` now returns
  the turns evicted from the front, and `record_conversation_turn` folds each into the
  conversation's `relux_core::ConversationSummary` via the pure, deterministic
  `prime_history::fold_evicted_turn`: an *acting* turn contributes a redacted highlight (the ids it
  created, bounded to `MAX_SUMMARY_HIGHLIGHTS = 16`, oldest dropped), a purely conversational turn
  contributes only to a count, and the first evicted turn seeds a single `opened_with` anchor. The
  summary is persisted per conversation (`KernelState.conversation_summaries`, the same `meta`
  snapshot seam, evicted alongside the ring under the conversation cap and survives a snapshot
  round-trip).

- **Where it surfaces.** `recent_conversation_context` renders the summary at the TOP of the SAME
  fenced BACKGROUND block, before the verbatim recent turns (`render_context_with_summary`), capped
  at `MAX_SUMMARY_RENDER_CHARS = 600` with a `[summary truncated]` marker — OpenClaw's summary +
  kept-entries shape. The empty-memory decision prompt is byte-for-byte unchanged (no summary + no
  ring → `""`), so the deterministic path is untouched. No new wire/dashboard field: the existing
  Prime **Clear** (`POST /v1/relux/prime/reset` → `clear_conversation`) now also drops the rolling
  summary.

- **Why it is safe / honest.** The summary is advisory prompt context with ZERO authority, exactly
  like the ring it compacts — never read by `classify_intent`, the fail-closed `reconcile_intent`
  gate, or any existence/approval check (those run on the CURRENT message alone), so even a summary
  full of "created task_XXXX" highlights can never promote casual chat into work
  (`a_summary_full_of_actions_still_never_promotes_casual_chat_into_work`). It is built ENTIRELY
  deterministically (no provider call — folding runs under the kernel lock) from data already
  redacted on the `ConversationTurn`: only ids + counts + the opening message, never a raw
  envelope, tool body, or secret; every field re-runs through `sanitize_text` defensively.

- **Still missing (honest).** A brain-generated (vs deterministic) summary — deferred as a
  strictly-additive, strictly-validated, off-lock overlay with a deterministic fallback and no
  unbounded calls — and cross-session recall (no FTS/search over prior conversations); both remain
  §6 P2.

## 17. Implemented this round — minimal scoped permission grants (§5 P1)

- **Reference read (BINDING).** OpenClaw `src/acp/permission-relay.ts`
  (`GatewayExecApprovalDecision = allow-once | allow-always | deny`, `buildAcpPermissionOptions` /
  `resolveGatewayDecisionFromPermissionOutcome` — the governance vocabulary of widening a grant from
  one-shot to standing, and the deny default) and OpenClaw `extensions/tlon/src/monitor/authorization.ts`
  (`resolveChannelAuthorization` → `{ mode: "restricted" | "open", allowedShips }`: a rule resolves an
  **allowlist**, membership decides, and the default is **restricted** — fail-closed scope matching).
  Paperclip's richer `(principal, permissionKey, scope)` model with `scopeAllows` + `agentIsInSubtree`
  is summarized in §5 from the original audit read; that source is **not vendored** under `reference/`,
  so only the minimal, self-containable half (a per-plugin tool scope, no subtree graph) was taken this
  round. Relux files read/mapped: `crates/relux-core/src/permission.rs`,
  `crates/relux-kernel/src/state.rs` (the `agent_holds_permission` chokepoint + `start_run` check +
  grant/revoke), `crates/relux-kernel/src/server.rs` (`/v1/relux/agents/:id/permissions`),
  `apps/dashboard/src/governance.ts` + `apps/dashboard/src/pages/Crew.tsx` (the Crew Governance panel).

- **Scoped syntax.** Exactly one new grant shape is accepted: `tool:<plugin-id>:*` — a scope that
  authorizes every concrete tool in that one plugin. `<plugin-id>` is `[A-Za-z0-9][A-Za-z0-9_-]*`. A
  `*` in any other position is rejected fail-closed (`*`, `tool:*`, `tool:*:*`, `agent:<id>:*`, partial
  globs like `tool:p:cre*`, a glob inside the plugin id), as is any path-like / injection string
  (whitespace, `/`, `\`, `..`). All existing exact capability strings are byte-for-byte unaffected.

- **Enforcement.** Grant-vs-required authorization moved from `matches_exact` to
  `relux_core::Permission::authorizes` at the two enforcement reads: `agent_holds_permission` (the one
  chokepoint every tool-invocation check — invoke, approve, per-tool-call binding, Prime turn — routes
  through) and the `start_run` task `required_permissions` loop. `authorizes` returns true iff the grant
  equals the required string OR the grant is a `tool:<plugin>:*` scope and the required is a concrete
  `tool:<plugin>:<tool>` in the **same** plugin. A scope never authorizes another scope, never crosses
  plugins (a plugin id that is a *prefix* of another does not match), and never matches a non-`tool:`
  capability. Plugin install / permission grants remain approval-gated exactly as before — `authorizes`
  is read-only and changes nothing about who may *issue* a grant.

- **Grant / revoke stay exact.** Grant dedup (`PermissionAlreadyGranted`) and revoke
  (`revoke_permission_from_agent`) still use `matches_exact`, so a scope is stored, displayed, and
  revoked as one explicit row and a revoke never pattern-expands into the concrete tools it covered
  (revoking a concrete tool an agent only holds *via* a scope is an honest `PermissionNotGranted`).

- **UI.** `governance.ts` mirrors the backend grammar (accepts `tool:<plugin>:*`, rejects every broader
  glob + path-like string with a scope-specific reason) and adds `isScopedWildcard` /
  `pluginWildcardPermission(pluginId)`. The Crew Governance panel explains the exact-vs-scope rule,
  shows a `scope: all tools in plugin` badge on scoped rows, and accepts the scope in the add field.
  No fake budget controls were added.

- **Tests.** `permission.rs`: grammar (accept the scope, reject broad/partial/non-tool globs +
  path-like strings) and authorization (exact authorizes only itself; scope authorizes every tool in
  its plugin; no overmatch across plugins / prefixes / kinds / wildcard-vs-wildcard).
  `state.rs::scoped_wildcard_grant_authorizes_plugin_tools_and_revokes_exactly`: a scoped grant
  authorizes the plugin's tools through `agent_holds_permission`, not a different plugin, and revoke
  removes exactly the scoped row. `governance.test.ts`: client-side validation + helper parity. Full
  `relux-kernel` `state::` suite (229) + `relux-core` permission suite green; clippy clean on both
  crates; dashboard typecheck + governance tests + bundle rebuild green.

- **Still missing (honest).** Agent-subtree / namespace / project scope (the larger Paperclip
  `scopeAllows` + `agentIsInSubtree` half — needs the §3 `reports_to` graph), governed budgets (§5 P1
  #3), and persistent `allow-always` grants (§5 P2) all remain open.

---

## 18. Implemented this round — the `reports_to` org-lattice / chain-of-command model (§3 P2)

- **Reference read (BINDING).** Paperclip's `reportsTo` org tree
  (`packages/db/src/schema/agents.ts`, indexed `(companyId, reportsTo)`) + `authorization.ts`
  `agentIsInSubtree` (a 50-depth upward walk) are the target, summarized in this audit's §3/§5 from
  the original read; that source is **not vendored** under `reference/`, so only the bounded-walk
  *shape* (not any scope enforcement) was taken. The **vendored** reads that ground the parent-pointer
  + bounded-depth + fail-narrow discipline: OpenClaw `reference/openclaw-main/src/acp/session-lineage-meta.ts`
  (`parentSessionId = parentSessionKey ?? spawnedBy`, a non-negative bounded `spawnDepth`,
  `subagentControlScope: "children" | "none"` — a node's authority is its children subtree or nothing,
  default narrow) and Hermes `reference/hermes-agent-main/tools/delegate_tool.py` (`MAX_DEPTH = 1`,
  per-record `parent_id`/`depth`, default flat). Relux files read/mapped: `crates/relux-core/src/agent.rs`
  (the `Agent` record), `crates/relux-kernel/src/agent_config.rs` (manual create/edit validation),
  `crates/relux-kernel/src/state.rs` (`create_agent*`/`update_agent*`), `crates/relux-kernel/src/server.rs`
  (`/v1/relux/agents` create/edit/list), `apps/dashboard/src/pages/Crew.tsx` (the Crew form + cards).

- **Model.** `relux_core::Agent` gained an optional `reports_to: Option<AgentId>` (the **Lead** in the
  lexicon; the internal id stays `reports_to` per the two-layer rule). `#[serde(default)]` makes every
  pre-existing snapshot load as a top-level operative (backwards compatible; pinned by core tests).

- **Pure helpers.** New `crates/relux-core/src/hierarchy.rs` — `chain_of_command` (the Line, nearest
  Lead first), `is_in_subtree` (proper-descendant: a node is not in its own Branch), and
  `would_create_cycle` (self OR target already in the child's Branch). Every walk is bounded by
  `MAX_HIERARCHY_DEPTH = 50` (Paperclip's depth) and guards against repeats, so it is **total even on a
  malformed/cyclic map**. These are the helpers a future manager-subtree scoped permission will read;
  **nothing reads them for authorization today** — enforcement is unchanged.

- **Validation (acyclic at the config boundary).** Create/edit resolve a requested Lead against the
  live roster: it must exist and cannot be self (`agent_config::resolve_manager` →
  `ReportsToUnknown`/`ReportsToSelf`). The kernel owns the graph invariants under its lock: a created
  operative is a fresh leaf (existence + self is the whole check); an **edit additionally rejects a
  cycle** via `hierarchy::would_create_cycle` (re-pointing a manager under its own report is refused).
  All failures surface as honest `400`s.

- **Behavior is display-only this round (safe).** The lattice is shown on the Crew card (each
  operative's Lead + a compact direct-report count) and drives the create/edit **Reports to (Lead)**
  picker (which excludes self + the operative's own Branch so an obvious cycle can't be chosen; the
  backend re-validates regardless). It is **not** used to widen any permission, and orchestration /
  assignment routing is deliberately untouched — keeping enforcement exactly as it was until a tightly
  scoped, separately-tested slice wires `is_in_subtree` into a grant.

- **UI.** `apps/dashboard/src/hierarchy.ts` (pure, mirrors the backend) — `descendantIds`,
  `managerOptions` (self + Branch excluded), `leadLabel`, `directReportsSummary`. `Crew.tsx` adds the
  Lead picker + the manager/direct-report card lines. `ReluxAgent`/`ReluxAgentConfig` gained
  `reports_to` (+ list-only `reports_to_name`/`reports`).

- **Honest disabled-target decision.** A Lead may be a `Paused`/`Disabled` operative — status and the
  org lattice are orthogonal (you can reorganize under a temporarily-disabled manager), and since the
  lattice grants no authority this round there is no safety reason to forbid it. The picker offers any
  non-self, non-Branch operative regardless of status; if that ever feeds a scoped grant, the grant
  (not the edge) is where a disabled-Lead check would live.

- **Tests.** `relux-core`: `agent.rs` backcompat (missing `reports_to` → `None`) + round-trip;
  `hierarchy.rs` chain order, subtree true/false/self, cycle (self/direct/transitive/idempotent),
  totality under a cyclic map, depth cap. `relux-kernel`: `agent_config` create/edit resolve + reject
  unknown/self; `state.rs` create stores/rejects, update set/clear, and **cycle rejection**.
  `apps/dashboard/test/hierarchy.test.ts` for the pure UI helpers; the existing Crew render harness
  exercises the new form/cards. Full `relux-core` (151) + `relux-kernel` (lib 628 / bin 109) suites
  green; clippy clean on both crates; dashboard typecheck + tests (284) + bundle rebuild green.

- **Still missing (honest).** The manager-subtree **scoped permission enforcement** (a grant that
  consults `is_in_subtree` — Paperclip `scopeAllows` + `agentIsInSubtree`) **SHIPPED in §19** (one
  narrow real path: a live manager granting a permission to a subordinate). Subagent
  spawn-depth/children caps, Codex/mid-run resume, governed budgets, and persistent `allow-always`
  grants all remain open.

---

## 19. Implemented this round — the manager-subtree scoped permission grant (§5 P1 / §18 follow-up)

- **Reference read (BINDING).** The manager-subtree authority target is Paperclip's
  `principal_permission_grants` with scope = `managerAgentId-subtree`, resolved by `authorization.ts`
  `scopeAllows` + `agentIsInSubtree` (a bounded upward `reportsTo` walk) — summarized in this audit's
  §5/§18 from the original read; that source is **not vendored** under `reference/`, so only the
  *shape* (a per-grant subtree scope, membership decided by the bounded walk, default-narrow) was taken.
  The **vendored** reads that ground the fail-narrow, self-scope-only discipline: OpenClaw
  `reference/openclaw-main/src/acp/session-lineage-meta.ts` (`subagentControlScope: "children" | "none"`
  — a node's authority is its children subtree or nothing, default narrow), OpenClaw
  `reference/openclaw-main/src/acp/permission-relay.ts` (deny-by-default, an explicit option set widens a
  grant), and Hermes `reference/hermes-agent-main/tools/delegate_tool.py` (`MAX_DEPTH`, per-record
  `parent_id`/`depth`, flat by default). Relux files read/mapped: `crates/relux-core/src/permission.rs`
  (grammar + matcher), `crates/relux-core/src/hierarchy.rs` (`is_in_subtree`), `crates/relux-core/src/agent.rs`
  (`reports_to`), `crates/relux-kernel/src/state.rs` (`grant_permission_to_agent`/`revoke`,
  `agent_holds_permission`, `reports_to_map`, `start_run` check), `crates/relux-kernel/src/server.rs`
  (the `grant_agent_permission`/`revoke_agent_permission` handlers, the `require_session` auth guard +
  `session_user`, the `list_audit_events` shape — the seam the new `manager-grant` route follows),
  `apps/dashboard/src/api.ts` (the `reluxWork` grant/revoke client), `apps/dashboard/src/governance.ts` +
  `apps/dashboard/src/pages/Crew.tsx` (the Crew Governance panel).

- **Scoped syntax.** Exactly one new grant shape is accepted: `agent:<manager-id>:subtree:<action>` — a
  manager-subtree scope. `<manager-id>` and `<action>` are each `[A-Za-z0-9][A-Za-z0-9_-]*`. It carries
  **no `*`**: the manager id is always concrete, so there is no global `agent:*:subtree:*` form (a scope
  can never name "every manager's subtree"). `subtree` is a **reserved keyword** in the `agent:`
  namespace — any `agent:` string that uses it outside the strict 4-segment position (`agent:x:subtree`,
  `agent:x:subtree:a:b`, `agent::subtree:g`, `agent:subtree:g`, `agent:x:subtree:*`) is rejected
  fail-closed as a malformed scope, never stored as an opaque capability. All existing exact `agent:`
  capability strings (and every other prefix) are byte-for-byte unaffected.

- **Model / matcher (real, pure, tested).** `relux_core::Permission` gained `is_manager_subtree` /
  `agent_subtree_parts`, and a free `relux_core::permission::manager_subtree_authorizes(grant, holder,
  action, target, reports_to)` decides authority. It returns true iff the grant is a well-formed
  `agent:<manager>:subtree:<action>`, **the grant's manager id equals `holder`** (a manager only ever
  wields authority over its OWN Branch — a grant naming someone else's id authorizes nothing, no
  borrowing), the action matches exactly (no action glob), and `target` is a **proper descendant** of
  `holder` via the bounded `is_in_subtree` walk (self / siblings / ancestors / unrelated all fail; total
  even on a cyclic map).

- **Enforcement (one real, narrow path).** `KernelState::manager_grant_permission_to_subordinate(manager,
  target, permission)` is the one production mutation that consults `reports_to` for *authority*. It
  authorizes via the kernel chokepoint `manager_subtree_authorizes`, which layers a fail-closed
  **liveness** rule on the pure matcher: **only an `Active` manager wields subtree authority** (a
  `Draft`/`Paused`/`Disabled`/`Error` manager is denied — the documented disabled-manager decision; the
  lattice and an actor's live powers are orthogonal, and the safe default for *exercising* a power is to
  require the actor be live). On success it grants through the existing `grant_permission_to_agent`
  (exact-match dedup, audited `agent:grant_permission`); on failure it audits `agent:manager_grant_permission`
  = `Denied` and grants nothing. It does **not** widen the operator-console grant/revoke path (those stay
  kernel/operator actions with no actor gate) — it adds a strictly *narrower* agent-authority path.

- **HTTP/API surface (SHIPPED — operator-assisted).** `POST /v1/relux/agents/:id/manager-grant` (where
  `:id` is the **acting manager**; body `{ "target_id", "permission" }`) now invokes the primitive through
  `KernelState::manager_grant_permission_to_subordinate_as_operator`. The route sits behind the same
  `require_session` guard as every other control-plane route. The handler parses the permission
  (malformed → `400`), resolves the authenticated operator from the session, and calls the kernel; an
  unauthorized manager (no scope / not Active / target outside its Branch / unknown manager-or-target,
  since existence folds into the fail-closed authority check) is a `403` that grants nothing. On success
  it returns the **target's** updated explicit permission list. The grant of authority is unchanged — the
  real own-Branch + Active + scope check in `manager_subtree_authorizes`; the operator only supplies the
  request. Two audit rows are written: the inner agent-actor view (`agent:grant_permission` /
  `agent:manager_grant_permission`) **plus** an `operator:authorize_manager_grant` row (Success/Denied)
  naming the operator and carrying a `trust_boundary` note.

- **HONEST trust boundary (the true remaining gap).** This is an **operator-assisted** path, not a
  per-agent-authenticated one. Relux has **no per-agent auth identity** yet: a manager agent cannot
  present its own credential on an HTTP request. OpenClaw correlates authority to a real per-session
  identity (`reference/openclaw-main/src/acp/session-lineage-meta.ts`: `sessionKey` / `spawnedBy` /
  `parentSessionKey` / `subagentControlScope`), and its permission relay routes a request to a human who
  selects allow-once/allow-always/deny (`reference/openclaw-main/src/acp/permission-relay.ts`). Relux's
  analogue today is the dashboard **operator** standing in for the manager — the operator authorizes
  "grant *as* this manager", but **cannot widen** anything the manager itself could not do (the kernel
  re-checks own-Branch + Active + scope and 403s otherwise). The genuinely-missing piece is an
  authenticated agent actor (a per-agent session/token whose identity the kernel trusts as the manager),
  so a manager could drive its own grant without an operator in the loop. Until then the operator is the
  named, audited authorizer. Exact grants still authorize only themselves; revoke still removes exactly the
  stored row via `matches_exact` (a manager-subtree grant is one explicit, individually-revocable row).

- **UI.** `governance.ts` mirrors the backend grammar (`isManagerSubtree`, `managerSubtreePermission`,
  and a scope-specific rejection reason for malformed subtree strings) — the `agent:` prefix is already
  **elevated**, so a subtree grant shows the `elevated` warning and a new `scope: manager subtree` badge.
  The Crew Governance panel adds an **Advanced — manager scope** explainer with the
  `agent:lead-1:subtree:grant_permission` example and the own-Branch / live-manager rules. The panel now
  also offers a **"Grant as manager"** affordance, gated by the pure
  `governance.ts::managerGrantAvailability(manager, roster)` helper (mirrored from the backend gate): it
  appears only when the selected agent is Active, holds a `agent:<id>:subtree:grant_permission` scope over
  its **own** Branch, and has at least one operative in that Branch — otherwise it shows the honest
  unavailable reason (no scope / not Active / empty Branch). When available it offers a Branch-subordinate
  picker + a permission input and calls `POST /v1/relux/agents/:id/manager-grant`; a badge states "operator
  stands in (no per-agent auth yet)". The normal operator grant/revoke form is unchanged. Helper parity is
  pinned by `governance.test.ts` (`managerSubtreeActions` own-id-only + `managerGrantAvailability`
  available/no-scope/paused/empty-Branch cases).

- **Tests.** `permission.rs`: grammar (accept the scope, reject every malformed subtree variant +
  case-sensitivity of the keyword) and the matcher (subordinate allowed; self / sibling / ancestor /
  wrong-action denied; cannot borrow another manager's Branch; total under a cyclic map). `state.rs::
  manager_subtree_grant_enforces_branch_liveness_and_audits`: a live lead grants to a real subordinate
  (target now holds it, success audited); sibling / ancestor / self / unrelated all denied; a paused
  manager is denied (liveness); a manager with no subtree scope is denied; the denial is audited.
  `server.rs::manager_grant_to_subordinate_over_http_enforces_authority_and_audits`: the end-to-end HTTP
  path — a live scoped lead grants to its subordinate (`200`, target's list grows); a sibling target, a
  manager with no scope, and a paused manager are all `403`; a malformed permission is `400`; and the
  `operator:authorize_manager_grant` audit row (with its `trust_boundary` detail) is present.
  `governance.test.ts`: client-side validation + helper parity + elevated classification + the new
  `managerSubtreeActions` / `managerGrantAvailability` availability gate. Full `relux-core` (156) +
  `relux-kernel` lib (629) + `relux-kernel` bin/server (110) suites green; clippy clean on both crates;
  dashboard typecheck + tests (289) + bundle rebuild green.

- **Still missing (honest).** A **truly per-agent-authenticated** actor surface (a manager driving its own
  grant without an operator in the loop) **SHIPPED in §20** (a bounded per-agent access token authenticates
  the manager directly on `POST /v1/relux/agents/me/manager-grant`). The operator-assisted path above
  remains as the operator-console affordance. The **second** subtree action, `assign_task`, **SHIPPED in §21**
  (`POST /v1/relux/agents/me/assign-task`). Still open: more subtree *actions* than `grant_permission` /
  `assign_task` (e.g. revoke), project / namespace scopes, governed budgets, persistent `allow-always`
  grants, agent-driven token enrollment, and Board-style oversight.

---

## 20. Implemented this round — the first per-agent identity / access-token primitive (§19 follow-up / §5 P1)

- **Reference read (BINDING).** The per-agent-identity target is **Paperclip, which IS vendored** under
  `references/paperclip/`: `references/paperclip/server/src/agent-auth-jwt.ts` (`createLocalAgentJwt(agentId, …)`
  / `verifyLocalAgentJwt` — a per-agent credential whose subject `sub` is the agent id, bounded `exp`/`iat`,
  signed HMAC-SHA256 and verified with a **timing-safe** compare) and `references/paperclip/server/src/middleware/auth.ts`
  (on a valid token, `req.actor = { type: "agent", agentId: claims.sub, source: "agent_jwt" }`, rejecting a
  terminated/pending agent — the acting identity comes from the verified token's subject, never the body).
  The narrow-by-default discipline is grounded in the **vendored** OpenClaw
  `reference/openclaw-main/src/acp/session-lineage-meta.ts` (`subagentControlScope: "children" | "none"`)
  and `reference/openclaw-main/src/acp/permission-relay.ts` (deny-by-default). The proven Relux local
  pattern reused is `crates/relux-kernel/src/auth.rs`'s operator `SessionStore` (hash-at-rest a
  high-entropy opaque credential; atomic permission-restricted file; prune/revoke in place). Relux files
  read/mapped: `crates/relux-kernel/src/{auth.rs,state.rs,server.rs,main.rs,lib.rs}`,
  `crates/relux-core/src/{redact.rs,permission.rs,agent.rs,hierarchy.rs}`,
  `apps/dashboard/src/{api.ts,governance.ts,pages/Crew.tsx}`.

- **Token model (bounded, hashed, revocable).** New `crates/relux-kernel/src/agent_auth.rs` —
  `AgentTokenStore` mints an opaque `relux_agt_<64 hex>` token bound to a specific agent (the **subject**,
  Paperclip's `claims.sub`) with a public, non-secret `agt_<hex>` handle for display/revocation. Only the
  token's **SHA-256 hash** is persisted (`dashboard-agent-tokens.json`, gitignored, written through the
  same atomic, permission-restricted path as the admin credential); the raw token is returned **exactly
  once** at mint and never again. Relux mints an **opaque hashed token, not a signed JWT** — there is no
  multi-tenant verifier to satisfy and a hashed-at-rest opaque token is simpler to revoke and impossible to
  forge from the stored file. Every token carries a bounded, clamped TTL (`[60s, 90d]`, default 30d) and is
  individually revocable. The `relux_agt_` prefix is added to `relux_core::redact` so a leaked token is
  masked defensively.

- **Auth surface (a two-route allowlist; never the operator console).** A new `require_agent_token`
  middleware validates `Authorization: Bearer <token>`, resolves `AgentTokenIdentity { agent_id, token_id }`,
  and inserts it into the request extensions. It gates a deliberately tiny `agent_router`: `GET
  /v1/relux/agents/me` (self-info, incl. the agent's Branch direct-reports) and `POST
  /v1/relux/agents/me/manager-grant`. The acting agent is **always the token subject** — read from the
  validated identity, NEVER the path/body — so a token can only ever act as itself. An agent token is
  **never** accepted on an operator route (those only ever check the `relux_session` cookie), and unlike the
  operator middleware this surface has **no `RELUX_AUTH_DISABLED` bypass** (an agent's identity is
  meaningless without a real token). Operator-only mint/list/revoke live on the session-gated
  `POST/GET /v1/relux/agents/:id/tokens` + `DELETE /v1/relux/agents/:id/tokens/:token_id`.

- **The grant path, per-agent-authenticated (the gap §19 left open, now closed for one action).**
  `KernelState::manager_grant_permission_to_subordinate_as_agent(token_ref, manager, target, permission)`
  drives the SAME unchanged authority gate as the operator-assisted path (`manager_subtree_authorizes` —
  own-Branch + `Active` + `agent:<id>:subtree:grant_permission` scope), but with **no operator in the
  loop**: the kernel trusts the authenticated agent identity as the acting manager (Paperclip's
  `req.actor.agentId = claims.sub`). It adds one `agent:token_authenticated_manager_grant` audit row
  (Success/Denied) carrying the **public** `token_ref` for provenance — the raw token never reaches the
  kernel or any log. Authority is unchanged; only the actor (a per-agent token, not an operator) differs.

- **UI.** `apps/dashboard/src/api.ts` gains `listAgentTokens` / `mintAgentToken` / `revokeAgentToken` (+
  `ReluxAgentTokenMeta` / `ReluxMintedAgentToken`); `governance.ts` gains the pure `parseTokenTtlSecs`
  helper. The Crew Governance card adds an **Access tokens (per-agent auth)** panel: mint (label + optional
  lifetime-in-days), a **copy-once** box that shows the raw token exactly once with a Copy/Dismiss control
  and a "never shown again" warning, a list of live tokens' non-secret metadata (id · label · expiry), and
  a per-token Revoke. The stored token is never re-displayed.

- **HONEST trust boundary (what changed).** Relux now HAS a per-agent auth identity for the manager-grant
  action: a manager presents its own bounded token and the kernel attributes the request to it directly —
  the §19 "genuinely-missing piece" for this one path. It is **operator-MINTED, not agent-enrolled**: the
  local operator issues the credential (an agent cannot bootstrap its own first token), which is the correct
  posture for a local-first console. The token is narrow — it unlocks only the agent-self routes; it grants
  no authority of its own (the own-Branch + Active + scope gate is unchanged); and it never touches the
  operator console. This is **not** an internet auth system: loopback-only, single local operator, opaque
  hashed token (no JWT/OAuth/issuer machinery).

- **Tests.** `agent_auth.rs`: mint↔authenticate roundtrip + subject-scoping, **raw token never persisted
  (only its hash)**, agent-scoped revoke, expiry prune, TTL clamp, list-without-secrets, restart
  persistence, bearer-header parsing. `state.rs::agent_authenticated_manager_grant_enforces_authority_and_records_token_provenance`:
  a token-authenticated lead grants to its subordinate (Success, token-provenance audit with the public
  handle), an unrelated target is denied (audited Denied). `server.rs::agent_token_mint_authenticate_self_grant_and_revoke_over_http`:
  the end-to-end HTTP path — mint (copy-once warning, raw token shape), list (no raw token / hash), mint for
  unknown agent → 404, self-info auth success (shows the Branch), no/garbage token → 401, self manager-grant
  Success then out-of-Branch 403 + malformed 400, revoke then the same token → 401, unknown-token revoke →
  404, and the mint/revoke/token-grant audit rows with the raw token absent.
  `server.rs::an_agent_token_does_not_open_operator_routes`: an agent token is 401 on `/state`, `/agents`,
  `/audit`, and the operator mint route still needs a session. `redact.rs`: the `relux_agt_` prefix is
  masked. `governance.test.ts`: `parseTokenTtlSecs`. Full `relux-core` (157) + `relux-kernel` lib (638) +
  bin/server (112) suites green; clippy clean on both crates; dashboard typecheck + tests (290) + bundle
  rebuild green.

- **Still missing (honest).** Agent-driven token **enrollment / rotation** (an agent minting or rotating
  its own credential — today the operator mints); more subtree *actions* than `grant_permission` (the
  second action, `assign_task`, **SHIPPED in §21**; `revoke` and others still open) and a richer agent
  self-service surface; project / namespace scopes; governed budgets; persistent `allow-always` grants;
  and Board-style oversight all remain open. The token is opaque-hashed, not a verifiable JWT — fine for
  a local single-operator console, but it does not federate across hosts.

---

## 21. Implemented this round — a second manager-subtree action: token-authenticated `assign_task` (§20 follow-up / §5 P1)

- **Reference read (BINDING).** The target is Paperclip's `principal_permission_grants` with scope =
  `managerAgentId-subtree`, resolved by `authorization.ts` `scopeAllows` + `agentIsInSubtree` (the same
  bounded `reportsTo` walk §19 mapped), only here the `permissionKey` is the **assignment** capability
  rather than the grant capability — a manager's authority over its Branch is *per-action*, not a single
  blanket power. Paperclip is **vendored** under `references/paperclip/`; the per-agent-actor attribution
  that drives it without an operator is `references/paperclip/server/src/middleware/auth.ts` (`req.actor =
  { type: "agent", agentId: claims.sub }`, the actor read from the verified token subject, never the body).
  The narrow-by-default discipline stays grounded in the **vendored** OpenClaw
  `reference/openclaw-main/src/acp/session-lineage-meta.ts` (`subagentControlScope: "children" | "none"`)
  and `reference/openclaw-main/src/acp/permission-relay.ts` (deny-by-default). Relux files read/mapped:
  `crates/relux-core/src/permission.rs` (the `manager_subtree_authorizes` matcher is already action-generic),
  `crates/relux-core/src/hierarchy.rs` (`is_in_subtree`), `crates/relux-core/src/task.rs`
  (`Task`/`TaskStatus`), `crates/relux-kernel/src/state.rs` (`manager_subtree_authorizes` chokepoint,
  `assign_task`, the §19/§20 grant primitives, `prime_update_slots::is_terminal_status`),
  `crates/relux-kernel/src/agent_auth.rs` (the per-agent token identity), `crates/relux-kernel/src/server.rs`
  (the `agent_router` bearer surface + the §20 `agent_self_manager_grant` handler), `crates/relux-kernel/src/lib.rs`
  (`KernelError`).

- **No new grammar.** The manager-subtree scope grammar (`agent:<manager-id>:subtree:<action>`) was
  already **action-generic** — `<action>` is any well-formed segment — so `agent:<id>:subtree:assign_task`
  parses, stores, and revokes exactly like `…:grant_permission` with **zero** change to `permission.rs`.
  The pure `manager_subtree_authorizes(grant, holder, action, target, reports_to)` matcher already takes
  the `action`, and a `…:grant_permission` scope authorizes ONLY `grant_permission` (and vice-versa) — no
  cross-action bleed (pinned by `permission.rs::subtree_grant_action_is_exact_and_generic_over_the_action_name`).

- **Enforcement (the second real subtree-authority path).**
  `KernelState::manager_assign_task_to_subordinate(manager, target, task)` is the second production
  mutation (after the §19 grant) that consults `reports_to` for *authority*, through the SAME kernel
  chokepoint `manager_subtree_authorizes(manager, "assign_task", target)` — own-Branch + **Active** manager
  + the exact `agent:<manager>:subtree:assign_task` scope. Authorization is checked **first** (an
  unauthorized manager never learns whether the task exists). On success it assigns through the unchanged
  `assign_task` (sets `assigned_agent`, moves the task to `Queued`, audited `task:assign`). It does **not**
  widen the operator/Prime assignment path — it is a strictly *narrower* agent-authority path.

- **Assignment semantics (the simple model, documented).** Relux's `assign_task` is a single-pointer
  assignment: it sets `assigned_agent` and moves the task to `Queued`. The manager path adds exactly one
  guard — the task must EXIST and be **assignable**, i.e. NOT in a terminal state
  (`Completed`/`Failed`/`Cancelled`/`Expired`, via `prime_update_slots::is_terminal_status`). A live but
  already-assigned task is simply re-pointed (the same semantics the operator/Prime path has). A terminal
  task is a resolvable conflict (`KernelError::TaskNotAssignable` → **409**), a missing task is the
  kernel's existing `UnknownTask` (**400**, unchanged from every other task route), and an unauthorized
  manager / unknown-or-out-of-Branch target is a **403** that assigns nothing — every denial audited.

- **Per-agent-authenticated surface.** `POST /v1/relux/agents/me/assign-task` (body `{ "task_id",
  "target_agent_id" }`) rides the §20 `require_agent_token` bearer middleware on the tiny `agent_router`
  allowlist. The acting manager is **always the token subject** (`AgentTokenIdentity.agent_id`), read from
  the validated token and NEVER from the body, so a token can only ever assign *as itself*. The handler
  calls `KernelState::manager_assign_task_to_subordinate_as_agent(token_ref, manager, target, task)`, which
  drives the unchanged authority gate and adds one `agent:token_authenticated_manager_assign_task` audit
  row (Success/Denied) carrying the **public** `token_ref` (the raw token never reaches the kernel or any
  log). An agent token is **never** accepted on an operator route (pinned by the extended
  `an_agent_token_does_not_open_operator_routes` boundary check), and operator routes are never reachable
  with a bearer. On success the route returns the updated `TaskRecord`.

- **HONEST trust boundary (unchanged from §20).** This adds a second *action* to the per-agent-authenticated
  manager surface; it grants no new authority shape. The token is operator-MINTED (an agent cannot enrol
  itself), opaque-hashed (not a JWT), loopback-only, single-operator. Authority is still own-Branch + Active
  + the exact scope; the only thing that changed is that a manager may now exercise **assignment** over its
  Branch (not just permission-granting), each scoped and individually revocable as its own capability row.

- **UI (SHIPPED — the deferred affordance, built honestly).** The Crew Governance card gains a compact
  **Manager actions (token-authenticated)** panel (`ManagerTokenActionsPanel` in
  `apps/dashboard/src/pages/Crew.tsx`), placed under the §20 Access-tokens panel. It (1) documents BOTH
  agent-self routes a token unlocks — `POST /v1/relux/agents/me/manager-grant` (`grant_permission`) and
  `POST /v1/relux/agents/me/assign-task` (`assign_task`) — spelling out the required
  `agent:<this-agent>:subtree:<action>` scope and the own-Branch + Active rule the kernel re-checks; (2)
  offers copy-paste **curl snippets** that embed NO secret — the token is referenced as the
  `$RELUX_AGENT_TOKEN` shell variable, never inlined; and (3) provides a collapsible **local test form** for
  **each** action — `assign-task` AND `manager-grant` (the grant form added as the §21 follow-up, so BOTH
  token-authenticated routes can be exercised from Crew, not just the documented snippet). Each form is
  honest about the trust boundary: because the raw token is shown copy-once and
  stored only as a hash, the dashboard **cannot** replay a minted token, so the operator must **paste it
  deliberately** into a `type="password"` field (cleared from state the moment the request returns), and each
  form keeps its **own** pasted token. The forms drive the per-action API helpers
  `agentSelfAssignTask(token, task_id, target_agent_id)` and
  `agentSelfManagerGrant(token, target_id, permission)` (`apps/dashboard/src/api.ts`), each of which sends
  `Authorization: Bearer <token>` and **`credentials: "omit"`** so
  the operator's `relux_session` cookie plays no part — it is the genuine per-agent bearer path, never the
  operator standing in. A 401/403 on this path means a bad/expired **token** (not an operator-session lapse),
  so it throws an honest `ApiError` and deliberately does **not** fire the dashboard's session-expired
  signal. The grant form validates the permission against the SAME backend grammar the add-permission form
  uses (`permissionInvalidReason`) before the request, so a malformed capability is caught client-side, and
  its trust-boundary copy spells out that the **token subject** is the acting manager and the operator cookie
  cannot stand in. Pure helpers live in `apps/dashboard/src/governance.ts` (`assignTaskFormReason`,
  `managerGrantFormReason`, `agentTokenLooksValid`, `assignTaskCurlSnippet`, `managerGrantCurlSnippet`, and
  the route constants). The
  panel never widens authority — it is a thin client over the documented route; the kernel remains the sole
  authority. **Trust boundary (UI):** the operator pastes a credential it cannot otherwise obtain, the bearer
  (not the cookie) authenticates, the acting manager is always the token subject, and no raw token is ever
  stored, re-displayed, or embedded in a snippet.

- **Tests.** `permission.rs::subtree_grant_action_is_exact_and_generic_over_the_action_name` (an
  `assign_task` scope authorizes only `assign_task`, no bleed with `grant_permission`, self never a target).
  `state.rs::agent_authenticated_manager_assign_task_enforces_authority_and_assignability`: a token-auth lead
  assigns a live task to its subordinate (target assigned + `Queued`, `task:assign` + token-provenance audit
  with the public handle); sibling / ancestor / self / unrelated all denied; a no-scope manager denied; a
  missing task is `UnknownTask`; a terminal (completed) task is `TaskNotAssignable` and left untouched; a
  paused manager wields no authority; the token-authenticated denial is audited.
  `server.rs::agent_token_assign_task_to_subordinate_over_http`: the end-to-end HTTP path — self-assign
  success (200, task assigned + queued), sibling/ancestor/self/unrelated 403, no-scope manager 403, unknown
  target 403, missing task 400, paused manager 403, and the `agent:token_authenticated_manager_assign_task`
  + inner `task:assign` audit rows present with the raw token absent; the operator-route boundary check now
  also asserts the new route is bearer-gated. Full `relux-core` (159) + `relux-kernel` lib (639) + bin/server
  (113) suites green; clippy clean on both crates. **Frontend (UI slice):** `governance.test.ts` pins
  `agentTokenLooksValid`, `assignTaskFormReason`, the new `managerGrantFormReason` (which validates the
  permission against the backend grammar — blank/`Must start with`/wildcard rejected, well-formed accepted),
  and that both curl snippets hit the real routes with the
  right body field names while embedding **no** raw token (the `$RELUX_AGENT_TOKEN` var only).
  `manager-token-actions.test.ts` stubs `fetch` and pins BOTH per-agent request shapes — `agentSelfAssignTask`
  (`POST` agent-self route, `credentials: "omit"`, `Bearer` header, body of only `{task_id,target_agent_id}`)
  and `agentSelfManagerGrant` (same route family, body of only `{target_id,permission}`) — and that a 403 on
  either throws an `ApiError` WITHOUT firing the session-expired signal.
  `manager-token-actions-render.test.mjs` server-renders the real panel (both routes documented, the required
  scope shown, a `type="password"` paste field per form, the bearer-var snippet, the Branch target picker,
  BOTH `Test … with a token` forms + `Assign as manager` / `Grant as manager` buttons, the grant form's
  permission field + token-subject trust-boundary note, **no raw
  `relux_agt_<chars>` token in the markup**) and asserts the committed bundle carries both panel buttons (no
  stale dist). Dashboard typecheck + tests (305) + bundle rebuild green.

- **Still missing (honest).** More subtree *actions* still open (`revoke`, status changes, …); agent-driven
  token enrollment/rotation; project / namespace scopes; governed budgets; persistent `allow-always` grants;
  a richer agent self-service surface; a *full* manager console (Board-style oversight, live task pickers) —
  the §21 UI now exercises BOTH token-authenticated routes (assign-task + manager-grant) from compact honest
  test affordances, but it is still not a Board-style console; that oversight surface remains open.

---

*Maintenance: when a slice from this audit ships, update its row in the §"Top P0/P1 gaps" table and
the dimension's status, and record the reference read in `reference-driven-development.md`. Keep the
status honest — never mark a dimension implemented if the code does not actually do it.*
