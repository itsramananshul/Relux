# REFERENCE_CODE_MAP — where to read the raw reference code before building

**BINDING (see `docs/reference-driven-development.md`).** Before changing Prime, the
brain loop, intent classification, tool planning, tool execution, approvals/grants, MCP,
or task/workflow behavior, you MUST read the corresponding raw reference source — NOT a
distillation, NOT this map alone. This file is a *navigation index* so you know which raw
files to open; it does not replace reading them. Quote the exact logic you learned and how
Relux maps it in your change writeup.

## Where the raw clones live (verified 2026-06-11)

| Reference | Canonical path (CLAUDE.md) | Mirror path | Status |
| --- | --- | --- | --- |
| **Hermes Agent** | `reference/hermes-agent-main/` | `references/hermes-agent/` | present (Python) |
| **OpenClaw** (Paperclip core) | `reference/openclaw-main/` | — | present (TypeScript) |
| **Paperclip** | `references/paperclip/` | — | present (TS; openclaw-derived) |
| **Open WebUI** | `reference/open-webui-main/` | — | present (UI only; not used for Prime/MCP) |

`reference/` also holds the original `*.zip` snapshots. Both `reference/` and `references/`
are gitignored and never tracked. The CLAUDE.md-cited canonical paths are
`reference/hermes-agent-main/` and `reference/openclaw-main/`; prefer those.

If a path above is ever missing, state exactly which path you checked and that it is
missing — do not silently fall back to a distillation.

---

## Hermes (`reference/hermes-agent-main/`, Python)

### `agent/conversation_loop.py` — the per-turn agentic tool loop
Read for: **loop continuation, the bounded tool-call iteration cap, the valid-tool gate.**
- `run_conversation(...)` drives the loop: `while (api_call_count < agent.max_iterations and
  agent.iteration_budget.remaining > 0) ...` (L598). Each round the assistant reply is
  inspected for `tool_calls` (L630, L854); a tool result is fed back as a `role:"tool"`
  message (L634, L676); when the model stops requesting tools the loop ends with its answer.
- The model can only call a tool in `agent.valid_tool_names` (L389, L656); an off-list name
  is fed back as a self-correction message, never executed.
- **Relux mapping:** `crates/relux-kernel/src/prime_tools.rs` — `ContextLoop` (the bounded
  driver, `MAX_TOOL_ROUNDS`), `interpret_reply` (tool-call detector), `classify_tool`
  (allowlist gate), `unknown_tool_feedback` (self-correction). The unified-decision path
  (`prime_decision.rs`) collapses this to one envelope; the kernel still validates and
  executes deterministically.

### `tools/mcp_tool.py` — the MCP client (list + call shaping)
Read for: **MCP `tools/list` discovery, `tools/call` result shaping, description scanning.**
- `_scan_mcp_description(server, tool, description)` (L367) scans an untrusted tool
  description for prompt-injection patterns and logs findings — **advisory, never a block**.
- Tool-call results: text content blocks are concatenated and `structuredContent` carried
  alongside; an `isError` result becomes an error, never a fabricated success (the no-raw-
  envelope contract).
- **Relux mapping:** `crates/relux-kernel/src/mcp.rs` — `discover_tools` (`tools/list`),
  `call_tool` (`tools/call`, shaped result, `ToolCallError` on `isError`); `relux_core::
  scan_mcp_tool_description` is the advisory description scan used in
  `KernelState::discover_mcp_tools`.

### Other Hermes files worth knowing
- `agent/tool_executor.py`, `agent/tool_guardrails.py`, `agent/tool_result_classification.py`
  — execution + guardrails + result classification.
- `hermes_cli/mcp_config.py`, `mcp_serve.py`, `agent/transports/hermes_tools_mcp_server.py`
  — MCP server config/serve surfaces.

---

## OpenClaw / Paperclip (`reference/openclaw-main/`, TypeScript)

### `src/tools/execution.ts` — the canonical tool-ref namespaces
Read for: **the stable tool-reference shape that keeps MCP tools from colliding with plugins.**
- `formatToolExecutorRef(ref)` renders `core:<id>`, `plugin:<pluginId>:<toolName>`,
  `channel:<id>:<action>`, and **`mcp:<serverId>:<toolName>`** (L12) — MCP tools live in a
  dedicated `mcp:` namespace.
- **Relux mapping:** `relux_core::mcp_synthetic_plugin_id(server)` → `mcp:<server>`, and a
  task `tool_plan` step's `(plugin = "mcp:<server>", tool)` (`relux_core::task::TaskToolCall`).
  Prime's plan-proposal parser (`crates/relux-kernel/src/prime.rs` `parse_tool_request`)
  recognizes an explicit `mcp:<server>/<tool>` token; the grounded step reuses the SAME
  `mcp:<server>` execution path as an installed-plugin step (no second tool system).

### `src/agents/tool-mutation.ts` — the fail-closed mutate/read classifier
Read for: **the fail-closed default — an UNKNOWN action is treated as mutating.**
- `isMutatingToolCall(toolName, args)` (L140): the `default` arm returns `true` for an
  unrecognized action (L165–L178); the recovery/fingerprint path also fails closed
  ("only clear when both fingerprints exist and match", L289–L295).
- **Relux mapping:** the polarity is inverted for the same safety. Prime's read-only context
  loop admits ONLY allowlisted read tools (`prime_tools.rs` `classify_tool` →
  `ToolKind::Refused` for anything else). For plan grounding, a referenced MCP tool that is
  not on the LIVE `tools/list`, or whose server is unreachable, grounds as
  `unknown` / `unavailable` (fail closed) — never silently accepted
  (`KernelState::build_tool_plan_proposal`, `crates/relux-kernel/src/state.rs`).

### `src/acp/approval-classifier.ts`
Read for: **approval/risk classification of a tool action** (maps to Relux's
`approval_blocks_direct_invocation` predicate + `McpToolClassification` default Medium+Required).
- An UNKNOWN tool → `autoApprove: false` (fail closed); a mutating/exec/control-plane tool
  never auto-approves. Relux mirrors the posture: an unclassified MCP tool is fail-closed
  Medium + Required, and a chat-staged gated call is never auto-approved.

### `src/acp/permission-relay.ts`
Read for: **the canonical three-decision approval model surfaced to a human** — the exact
shape a chat-initiated gated tool approval must offer.
- `GatewayExecApprovalDecision = "allow-once" | "allow-always" | "deny"`;
  `buildAcpPermissionOptions` renders them as "Allow once" / "Allow always" / "Deny", and
  falls back to `["allow-once", "deny"]` when allow-always is not applicable (L24, L50-77).
  The **approval id is the stable correlation key** for an early prompt (L141-150).
- **Relux mapping:** the Prime chat **approval card** (`apps/dashboard/src/pages/Prime.tsx`
  `ApprovalCard`) offers the SAME three decisions wired to the EXISTING routes — "Approve &
  run" (`decide:approved` → `execute`), "Allow always" (`allow-always` → `execute`), "Deny"
  (`decide:rejected`, which drops the bound invocation). The pending `ApprovalId` is the
  stable key; `PrimeToolApprovalRequest.allow_always_supported` gates the middle button
  exactly as `buildAcpPermissionOptions` gates `allow-always`. The kernel staging
  (`KernelState::request_tool_invocation_approval`) is the consume-once exec-approval
  register; nothing runs until the human decides (`docs/mcp.md` "Chat-staged approval").

---

## The slice this map was written for: live MCP tools in Prime plan proposals

Read order for that change:
1. `reference/openclaw-main/src/tools/execution.ts` (the `mcp:<server>:<tool>` ref shape).
2. `reference/hermes-agent-main/tools/mcp_tool.py` (`tools/list` discovery; `_scan_mcp_description`).
3. `reference/openclaw-main/src/agents/tool-mutation.ts` (fail-closed default).

Relux files that implement it (`docs/mcp.md` "Run-driven multi-tool plan"; `RELUX_MASTER_PLAN`
§10.5, §17.1):
- `crates/relux-kernel/src/prime.rs` — `parse_tool_request` recognizes `mcp:<server>/<tool>`.
- `crates/relux-kernel/src/state.rs` — `live_tool_catalog` (installed + live MCP merged,
  read-only; grounds BOTH the inert plan proposal AND a single explicit MCP invocation),
  `discover_proposal_mcp_catalog` (off-lock `tools/list`), `build_tool_plan_proposal`
  (fail-closed grounding), the transient `proposal_mcp_catalog` + `set_proposal_mcp_catalog`.
- `crates/relux-kernel/src/server.rs` — pre-fetches the catalog OFF-LOCK before the locked turn.
- `apps/dashboard/src/pages/Prime.tsx` — the inert proposal card (MCP badge, readiness/risk,
  args preview) and the existing tool-run task-create route.
