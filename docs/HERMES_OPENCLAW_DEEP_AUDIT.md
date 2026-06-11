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
| 2 | **Structured error/liveness classifier + bounded transient retry** — Relux retry is a fresh run with no error taxonomy and no backoff; Paperclip classifies (`run-liveness.ts`) and retries transient upstream failures on a bounded `[2m,10m,30m,2h]` schedule. | 7 | P1 | backend, tests |
| 3 | **Governed budgets (soft/hard, auto-pause)** — Paperclip enforces per-company/agent/project spend with warn + hard-stop + cancel-work. Relux records run `cost`/`usage` but enforces nothing. | 5 | P1 | backend, frontend, docs, tests |
| 4 | **Scoped permission grants (subtree / project)** — Relux permissions are exact-string match only; Paperclip has fine-grained grants scoped to manager-subtrees/projects. | 5 | P1 | backend, tests |
| 5 | **Memory compaction / cross-session recall** — Relux keeps a bounded 12-turn ring with no summarization; Hermes/OpenClaw compact + summarize + (Hermes) FTS5 cross-session search. Low urgency at current turn volume but blocks long-running Prime sessions. | 6 | P1/P2 | backend, tests |
| 6 | **`execute_code` (RPC-from-script deterministic glue)** — the cheapest multi-step primitive; routes back through the same tool gate. Big, but high-leverage. | 2, 4 | P1 | backend, tests, docs |
| 7 | **Goal/issue hierarchy + monitor/recovery** — Relux orchestration is a flat ≤6-step DAG; Paperclip has Goal→Project→Issue→Run with monitor scheduling + stranded-issue recovery. | 4 | P2 | backend, frontend, docs, tests |

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

### Relux mapping — **partial**

- `crates/relux-core/src/agent.rs` — `Agent` (id/name/description/adapter/persona/skills/status/
  permissions/namespace), `AgentStatus`. `crates/relux-kernel/src/agent_config.rs`,
  `agent_presets.rs` — manual crew config + role presets.
- Assignment/target resolution: `crates/relux-core/src/orchestration.rs` `resolve_assignee`
  (exact→prefix→substring against the live roster); skill-aware matching.
- **Durable agents** exist (they outlive the turn and run via the orchestration batch). **Missing**:
  an explicit `reports_to` org tree / chain-of-command, a session/handoff/resume concept beyond
  task-level fresh-run retry, and subagent spawn-depth/children caps (orchestration has step/concurrency
  caps instead).

### Priority & slices

- **P2 — `reports_to` chain-of-command + manager-subtree authority** (Paperclip): pairs with scoped
  grants (§5). The lexicon already reserves `reports_to` as a stable internal id. *(backend, tests, docs.)*
- **P2 — run resume/continuation** (OpenClaw `resumeSessionId`): resume a failed run from its prior
  session instead of a cold fresh run. Pairs with the error classifier (§7). *(backend, tests.)*

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
- **One-shot approval + fail-closed gate + per-tool approval are implemented.** **Missing**:
  budgets/spend enforcement (runs record `cost`/`usage` but nothing enforces a ceiling), scoped/subtree
  grants (permissions are **exact-string match only** — no project/subtree scope), persistent
  `allow-always` grants, Board-style multi-party oversight.

### Priority & slices

- **P1 — governed budgets** (`budget.rs` core type + kernel enforcement): per-namespace/agent soft
  warn + hard stop that pauses new runs and surfaces a Doctor/approval signal. Maps to Paperclip
  `budgets.ts`. *(backend, frontend, docs, tests.)*
- **P1 — scoped permission grants**: extend `Permission`/grant model with a scope (agent-subtree /
  namespace) instead of exact-match only. Pairs with §3 `reports_to`. *(backend, tests, docs.)*
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
- **Implemented**: bounded, redacted, advisory history + clarify memory. **Missing**: any
  summarization/compaction past the 12-turn eviction, and cross-session recall (no FTS/search).

### Priority & slices

- **P1/P2 — compaction/summarization beyond the ring**: when a conversation exceeds the ring, fold the
  evicted turns into a bounded redacted summary (OpenClaw `compact()` / Hermes 12-section). Low
  urgency now; blocks long-running Prime threads. *(backend, tests.)*
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

### Relux mapping — **partial / missing**

- `crates/relux-core/src/adapter_result.rs` — honest parse, plain-text fallback, never fabricates
  success/failure. `KernelError` taxonomy (UnknownTask/Agent, PermissionDenied, …).
- `crates/relux-kernel/src/state.rs` — run status FSM; `PrimeAction::RetryRun` is an **explicit fresh
  run** (no partial resume, no backoff, no classification).
- Brain loop: `DecisionLoop` stops on provider failure keeping the interim decision; until this slice,
  **no self-correction** of a malformed decision.
- **Missing**: an error/liveness classifier, automatic bounded transient retry with backoff,
  circuit-breaking, output-silence/stranded recovery.

### Priority & slices

- **P0 — self-correction on malformed decisions (SHIPPED, §1/§13).**
- **P1 — error classifier + bounded transient retry**: a `RunFailureClass` (transient-upstream /
  permission / config / fatal) + a bounded backoff retry for transient classes only, behind the
  existing governed run path. Maps to Paperclip `run-liveness.ts` + the `[2m,10m,30m,2h]` schedule.
  *(backend, tests.)*
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

*Maintenance: when a slice from this audit ships, update its row in the §"Top P0/P1 gaps" table and
the dimension's status, and record the reference read in `reference-driven-development.md`. Keep the
status honest — never mark a dimension implemented if the code does not actually do it.*
