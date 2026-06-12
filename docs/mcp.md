# MCP support (Relux — loopback discovery + gated invocation)

> Scope: the **relux-* product layer** (`relux-core` / `relux-kernel` / `apps/dashboard`).
> The legacy `relix-runtime` MCP scaffold (`docs/mcp-tool.md`, `crates/relix-runtime/src/nodes/tool/mcp*.rs`)
> is a SEPARATE, older surface and is unchanged by this slice.

Spec refs: `docs/RELUX_MASTER_PLAN.md` §8.2 (ToolSet Plugins) + §18 (no
auto-running of downloaded code); `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9
("P2 — MCP tool support").

## What ships

A safe Model Context Protocol surface for Relux: an operator-curated,
**loopback-only** MCP server **registry + live tool discovery + gated invocation**.
MCP tools are called through the **same** permission / approval / grant / audit
gates a real plugin tool uses — never a separate, weaker path.

An operator can:

- **Register** a loopback MCP server: `{ id, endpoint, description?, enabled?, timeout_ms? }`.
  The endpoint is validated with the SAME loopback-only rule as the plugin runtime
  (`relux_core::validate_loopback_url`): only `http://127.0.0.1|localhost|[::1]:<port>`
  with an explicit port is accepted. `https`, remote hosts, embedded credentials,
  query/fragment, and `..` paths are all rejected.
- **List** registered servers with an honest one-word status (`configured` /
  `disabled`). No secrets are ever stored or returned — only the id, endpoint,
  description, enabled flag, per-call timeout, and per-tool classifications.
- **Discover** an enabled server's tools: the kernel runs a live MCP `initialize`
  handshake followed by `tools/list` against the loopback endpoint and maps the
  result into the standard `relux_core::ToolDescriptor` shape
  (`plugin_id = "mcp:<id>"`, the enforced permission `tool:mcp-<id>:<verb>`, the
  classified risk, `source_kind = "Mcp"`). Each tool's `executable` is honest:
  an unclassified tool is `needs_approval` (gated), a classified low-risk
  auto-approve tool is `ready`.
- **Classify** a discovered tool's risk + approval, turning a gated tool into a
  directly-callable one (or keeping it gated). Until classified, a tool defaults to
  the fail-closed Medium + `Required` — never directly runnable.
- **Invoke** a tool through the standard tool-invoke surface (`plugin_id = "mcp:<id>"`):
  the kernel runs `initialize` → `tools/call` against the loopback server and returns
  a **shaped, sanitized** result. Every call is permission-checked, risk/approval-gated,
  per-call-approvable, persistent-grant-bypassable, and audited — identical to a plugin
  tool (see "Invocation" below).
- **Remove** a server registration (which also drops its tool classifications).

## Invocation (loopback `tools/call` through the gates)

MCP tools are **first-class tool-invoke citizens**: they flow through the kernel's
existing `call_tool` / `invoke_tool` / per-call-approval / persistent-grant path,
using the synthetic `plugin_id = "mcp:<server>"`. There is no separate MCP invoke
endpoint — the existing `/v1/relux/tools/invoke`, `/v1/relux/tools/request-approval`,
`/v1/relux/approvals/:id/execute`, `/v1/relux/approvals/:id/allow-always`, and
`/v1/relux/grants` surfaces all accept `mcp:<server>` as the plugin id.

**Permission model (enforced).** A discovered tool's required permission is
`tool:mcp-<server>:<verb>` (the server id sanitized; `<verb>` is the tool name's
trailing dotted segment). The calling agent must hold it (exact grant or the scoped
`tool:mcp-<server>:*` wildcard) — there is no broad MCP wildcard by default. The
kernel resolves this permission directly from the MCP server registry; it never
touches the installed-plugin manifest map.

**Risk / approval model.** A discovered MCP tool's real risk is unknown, so it
defaults to the fail-closed `McpToolClassification` (risk `Medium`, approval
`Required`). The SAME `approval_blocks_direct_invocation` predicate that gates a
plugin tool then decides: a gated tool is `needs_approval` (refused on the direct
invoke path; runnable only via a per-call approval or a standing allow-always
grant), and a tool the operator classifies as low-risk + `Never` becomes `ready`
(directly invocable, still permission-checked + audited). The operator sets a
classification per tool; clearing it reverts to the gated default.

**Invocation path (every safeguard reused).** On a `mcp:<server>` invocation the
kernel: (1) resolves the `tool:mcp-<server>:<verb>` permission and checks the agent
holds it; (2) applies the risk/approval gate (and the per-call / persistent-grant
bypass) exactly as for a plugin tool; (3) re-validates the loopback endpoint on
every call, then runs `initialize` → `notifications/initialized` → `tools/call` with
`{ name, arguments }` against the loopback server, bounded by the per-call timeout and
the request/response size caps; (4) **shapes** the result — text content blocks are
concatenated into `{ "result": <text>, "structuredContent"?: … }`, a `tools/call`
`isError` becomes an honest runtime failure, and the **raw JSON-RPC envelope is never
returned** to the UI; (5) audits the call (success / denial / failure) on the
append-only log. The tool name is re-validated as a safe identifier
(`is_valid_mcp_tool_name`) before any dial. No arbitrary downloaded code is run — only
the operator's own loopback MCP server is dialed.

## Session continuity (streamable-HTTP `Mcp-Session-Id`)

A streamable-HTTP MCP server may be **stateful**: its `initialize` response sets an
`Mcp-Session-Id` HTTP header, and it then rejects any later request that does not
echo that header (typically with a `400`/`404`). Relux now speaks this within a
single logical operation:

- On `initialize`, the kernel client captures the `Mcp-Session-Id` **response
  header** (if present), **validates** it (non-empty, ≤512 chars, visible-ASCII
  `0x21..=0x7E` only — the MCP-spec session charset) and **echoes** it as the
  `Mcp-Session-Id` **request header** on the operation's remaining requests
  (`notifications/initialized`, then `tools/list` or `tools/call`). A malformed /
  oversized value is **dropped, not echoed** — that closes a header-injection path
  (a hostile value cannot smuggle `CR`/`LF` into our next request) and the operation
  simply proceeds session-less.
- **Session invalidation** is handled once, bounded: if a request returns HTTP
  `404` **while a session id is held** (the streamable-HTTP "session expired /
  unknown" signal), the client clears the session, runs **one** fresh `initialize`
  to obtain a new session, and **retries the operation a single time**. If it still
  fails, the error is surfaced honestly (`McpClientError::HttpStatus(404)` →
  discovery/invocation failure) — there is no retry loop and no fabricated result.
- **The session id never leaves the kernel.** It lives only in an in-memory
  `Session` for the duration of one operation, is dropped when that operation ends,
  is **never persisted** (not in `McpServerConfig`, not in the snapshot), **never
  logged**, and **never returned** by discover/invoke — so it cannot reach the
  dashboard or the HTTP API. There is no cross-call session reuse: each discover /
  invoke opens, uses, and discards its own session.

## MCP resources (read-only context / Dossier source)

MCP **resources** are a second, read-only surface alongside tools: a server
advertises addressable context (files, records, docs) via `resources/list`, and a
client fetches one by URI via `resources/read`. Unlike a tool, a resource is
**inert** — reading it performs no action and mutates nothing — which is exactly why
it is safe to expose as a read-only context source to Prime and to operators.

- **Client (`relux-kernel::mcp`).** `list_resources(endpoint, timeout)` runs
  `initialize` → `resources/list`, and `read_resource(endpoint, uri, timeout)` runs
  `initialize` → `resources/read { uri }`. Both flow through the SAME loopback-only,
  bounded-timeout/size, session-continuous (`Mcp-Session-Id`) path as tool discovery —
  the endpoint is re-validated on every call, a stale-session `404` triggers one
  bounded re-initialize, and a transport/protocol failure or JSON-RPC `error` is an
  honest [`McpClientError`], never a fabricated list/body.
- **Validation + bounds.** A listed resource's `uri`/`name`/`title`/`mimeType`/
  `description` are sanitized and clamped (`MAX_MCP_RESOURCE_*`); at most
  `MAX_MCP_RESOURCES` (256) resources are returned. A `resources/read` URI must pass
  `relux_core::is_valid_mcp_resource_uri` (non-empty, ≤2048 chars, control-char free)
  or the read is refused before any dial (fail closed).
- **Content shaping (text-only, honest binary, redacted).** A `resources/read`
  result's `contents` text blocks are concatenated; a **binary (`blob`) block is
  summarized** with an honest `[binary content omitted: <mime>]` marker (its bytes are
  never decoded or returned) and the `binary` flag is set. The joined text is
  sanitized, **secret-redacted** (`relux_core::redact_secrets` — a credential embedded
  in a resource body never leaks verbatim), and clamped to
  `MAX_MCP_RESOURCE_TEXT_CHARS` (20 000). The raw JSON-RPC envelope is never returned.
- **Kernel state (read-only).** `KernelState::list_mcp_resources(id)` and
  `read_mcp_resource(id, uri)` are `&self` (no audit mutation, mirroring
  `discover_mcp_tools`): an unknown server → `UnknownMcpServer`; a disabled one →
  `McpServerDisabled`; an invalid URI → `InvalidMcpResourceUri`; a transport failure →
  `McpResourceFetchFailed`.
- **Prime read-only context integration.** Three read-only context tools join the
  governed [`READ_ONLY_TOOLS`] allowlist so a configured brain can request resource
  context inside the SAME bounded observe-then-act loop / unified-decision read path:
  `list_mcp_servers` (PURE — lists the registered loopback servers from the snapshot),
  `mcp_list_resources` (live `resources/list` for a `server_id`), and
  `mcp_read_resource` (live `resources/read` for a `server_id` + `uri`). The live two
  dial the loopback endpoint carried in the snapshot **outside the kernel lock**
  (exactly like the brain rounds), so the lock is never held across I/O. They are
  read-only — no mutation, no action authority — and use the existing read-only
  provenance ([`ContextRead`] → [`PrimeContextRead`]: tool + ok + bounded summary; the
  full body and the endpoint stay server-side). An unknown/disabled server or a
  transport failure is an honest `ok:false` read, never a fabricated body.

## Run transcript visibility (run-bound MCP calls)

An MCP **tool call made inside a run** is now visible on that run's transcript, not
only on the audit log. When a tool is invoked through the run-context chokepoint
`KernelState::call_tool` (the path that carries a `run_id`) and the plugin id is an
MCP server (`mcp:<server>`), the kernel appends a **distinct, bounded,
secret-redacted** run event:

- `mcp_tool_call` (success) — payload `{ server, tool, ok: true, result_summary }`,
  message `called MCP tool <tool> via mcp:<server>`. The `result_summary` is the
  shaped result's human `result` text, **secret-redacted** (`redact_secrets`) and
  clamped to 500 chars. The raw args, the `structuredContent`, the JSON-RPC
  envelope, and the streamable-HTTP session id are **never** put on the transcript.
- `mcp_tool_call_denied` — payload `{ server, tool, permission | reason }` for a
  permission denial or a requires-approval refusal.
- `mcp_tool_call_failed` — payload `{ server, tool, reason }` with a redacted reason
  for a transport/protocol/runtime failure.

A real plugin tool keeps its existing generic `tool_call` / `tool_call_denied` /
`tool_call_failed` events unchanged (those still carry the full input/output for a
trusted local tool); only the MCP branch gets the compact, redacted shape.

**Where it shows.** These events surface in the Work run detail's existing
**Transcript / Events** table (kind → label via `phaseLabel`, e.g. "MCP tool call")
and fold into the run header's tool-call summary (`toolCallSummary`) — no new UI
surface. (`apps/dashboard/src/runview.ts`, `apps/dashboard/src/pages/Work.tsx`.)

**No run context → audit only (honest).** The manual Plugins invoke path
(`invoke_tool`), the per-call **approval execute** path
(`execute_approved_tool_invocation`), and a persistent-grant bypass invoked outside
a run carry **no `run_id`**, so they cannot (and do not) append a run-transcript
entry — they remain fully recorded on the append-only **audit log** (which already
captures actor, action `tool:mcp-<server>:<verb>`, result, and the `via` / `grant` /
`approval` metadata). This is deliberate: a run transcript belongs to a run, and
these operator-initiated invocations are not part of one.

**Prime read-only MCP context.** When Prime's read-only context loop reads MCP
context (`list_mcp_servers`, `mcp_list_resources`, `mcp_read_resource`), the
provenance is the existing [`PrimeContextRead`] (tool + ok + bounded summary) shown
as the `🔎 used:` context-read chip — the full resource body and the endpoint stay
server-side. No raw resource body is stored on a turn. A Prime turn is not a run, so
it carries no run transcript; the context-read provenance is the bounded record.

## Run-driven MCP tool call (first production run path through `call_tool`)

The run-transcript events above were wired through `call_tool`, but until now **no
production run path routed an MCP tool call through it** — only the test suite and the
default local echo exercised the branch. A `Task` can now carry an explicit,
operator-named **tool-call directive** so a real run drives one MCP (or plugin) tool
through the gated chokepoint.

- **The directive.** A task's `input` may be the canonical shape
  `{ "tool_call": { "plugin": "mcp:<server>", "tool": "<name>", "args": { … } } }`
  (`relux_core::TaskToolCall` / `parse_task_tool_call`). `plugin` may be a synthetic
  `mcp:<server>` MCP server **or** a real installed plugin id — `call_tool` applies the
  identical gates to both. `args` defaults to `{}`. The directive is **fixed at task
  creation time**: the brain never chooses the tool, and there is no implicit
  tool-selection — an ordinary (non-directive) task still runs the default echo.
- **Execution.** When the deterministic local run (`KernelState::execute_local_run`,
  the `LocalPrime` adapter path behind "Run (Assigned)") sees a directive, it calls
  `self.call_tool(run_id, agent, plugin, tool, args)` **instead of** echo. That means
  the run-driven MCP call flows through the SAME path as every other tool call:
  (1) the `tool:mcp-<server>:<verb>` permission is resolved and checked against the
  assigned agent; (2) the risk/approval gate applies, with the per-call-approval and
  standing **allow-always grant** bypasses; (3) the loopback `tools/call` runs, shaped
  + bounded; (4) the call is audited; (5) the distinct `mcp_tool_call*` transcript
  event is appended (success carries only the bounded, secret-redacted
  `result_summary`).
- **Honest outcomes (never a fabricated success).** A directive whose tool is not
  runnable — the agent lacks the permission, the tool requires approval with no
  standing grant, or the loopback call fails — **fails the run and the task** and
  surfaces the gate's `mcp_tool_call_denied` / `mcp_tool_call_failed` event. A
  requires-approval directive blocks the run rather than auto-running; an operator must
  either classify the tool low-risk + auto-approve or stand up an allow-always grant
  for the call to proceed.
- **Operating it (no new UI).** An operator creates such a task over the existing
  `POST /v1/relux/tasks` route — which now accepts an optional `tool_call` body
  (validated; an empty plugin/tool is a `400`) and serializes it into the canonical
  input — then runs it with the existing "Run (Assigned)" /
  `POST /v1/relux/tasks/:id/execute-assigned` action. The resulting `mcp_tool_call`
  event surfaces in the Work run detail's existing Transcript table. No bespoke MCP-run
  UI is added in this slice.

**Scope (deliberately narrow).** This is a **deterministic, operator-named** run path:
one tool, fixed in the task input, gated end-to-end. It is NOT a brain freely choosing
arbitrary MCP tools mid-run — that broader autonomy stays out of scope until it routes
through an allowlisted/validated write tool and the same approval gates.

## What it does NOT do (honest limitations)

- **No stdio (command) MCP servers.** Relux never spawns arbitrary downloaded
  code. Only an operator-run loopback HTTP server is dialed.
- **No remote / `https` / SSE-subscription transport.** v1 dials no remote host.
- **Session-continuous streamable HTTP (within one operation).** Each JSON-RPC
  request is still its own `Connection: close` POST (`initialize`, then `tools/list`
  or `tools/call`), but a single logical operation now carries a **streamable-HTTP
  session** across its requests — see "Session continuity" below. A single
  SSE-framed response body (`data: {json}`) is parsed, so simple stateless
  streamable-HTTP servers also work. What is still NOT supported: a **long-lived
  SSE subscription / server-push channel**, and **cross-operation** session reuse
  (each discover/invoke runs its own `initialize`; sessions are never kept open or
  shared between calls).
- **No OAuth / auth header.** No `Authorization` is sent. (Hermes' `mcp_oauth_manager`
  is deferred.)
- **No MCP prompts / sampling.** `prompts/*` and server-initiated sampling are not
  spoken. **MCP resources ARE now supported** as a read-only context source —
  `resources/list` + `resources/read` (text content; binary summarized honestly). See
  "MCP resources" below.

Bounds (all reused from / mirrored on the loopback-tool runtime): per-call
connect/read/write timeout (clamped `100..60_000 ms`, default `10_000`), request
body cap (256 KiB), response body cap (1 MiB), at most 256 discovered tools per
server, descriptions sanitized + clamped to 600 chars and scanned for
prompt-injection patterns (advisory warning, never a block — mirrors Hermes
`_scan_mcp_description`). On invocation: the tool name must be a safe identifier
(`[A-Za-z0-9._-]`, ≤128 chars), the args are bounded by the per-call-approval args
cap, and the shaped result's text is clamped to 20 000 chars.

## HTTP surface

```
GET    /v1/relux/mcp/servers                                       list registered servers (no secrets)
POST   /v1/relux/mcp/servers                                       { id, endpoint, description?, enabled?, timeout_ms? } (upsert by id)
DELETE /v1/relux/mcp/servers/:id                                   remove a server (and its classifications)
GET    /v1/relux/mcp/servers/:id/tools                             live tools/list discovery → ToolDescriptor[]
GET    /v1/relux/mcp/servers/:id/resources                         live resources/list → McpResource[] (read-only context)
GET    /v1/relux/mcp/servers/:id/resources/read?uri=…              read one resource → shaped, redacted McpResourceContent
PUT    /v1/relux/mcp/servers/:id/tools/:tool/classification        { risk, approval } — set a tool's risk/approval
DELETE /v1/relux/mcp/servers/:id/tools/:tool/classification        revert a tool to the gated default
```

The resource routes are READ-ONLY (a read lock; no save). Honest status codes:
unknown id → `404`; disabled server → `409`; an invalid/empty `uri` → `400`; a
loopback transport/protocol failure → `502` (never an empty-list/empty-body lie).

MCP tools are **invoked through the standard tool-invoke surface** with
`plugin_id = "mcp:<server>"` — no separate MCP invoke route:
`/v1/relux/tools/invoke`, `/v1/relux/tools/request-approval`,
`/v1/relux/approvals/:id/execute`, `/v1/relux/approvals/:id/allow-always`, and
`/v1/relux/grants` all accept `mcp:<server>`.

Honest status codes: unknown id → `404`; disabled server discovery → `409`; a
loopback transport/protocol failure → `502` (never an empty-list lie); a
non-loopback / malformed config or invalid tool name → `400`; a permission denial →
`403`; a tool that requires approval (invoked directly) → `409`.

## Reference-driven mapping (`docs/reference-driven-development.md`, BINDING)

Files read before implementing:

- **Hermes** `reference/hermes-agent-main/tools/mcp_tool.py` — the MCP wire shape
  (`initialize` → `tools/list` → tool `{ name, description, inputSchema }`;
  `tools/call` with `{ name, arguments }` → `CallToolResult { content, isError,
  structuredContent }`, L2334-2382). We mirror its **result shaping**: collect text
  content blocks into `result`, carry `structuredContent` alongside, and treat
  `isError` as an honest failure — never returning the raw envelope.
  `_validate_remote_mcp_url` (http(s) + real host; we go stricter → loopback only),
  `_scan_mcp_description` + `_MCP_INJECTION_PATTERNS` (advisory injection scan we
  mirror dependency-free), per-server `timeout`/`connect_timeout` (we clamp).
  `reference/hermes-agent-main/hermes_cli/mcp_config.py` — `mcp_servers` config map
  keyed by id, `add/remove/list/test` lifecycle (we mirror register/list/remove +
  live discover + classify).
- **Relix legacy** `crates/relix-runtime/src/nodes/tool/mcp_http.rs` — the prior
  streamable-HTTP MCP client: one POST per JSON-RPC request, `ensure_initialized`
  before `list_tools`/`call_tool`, `tools/call` params `{ name, arguments }`,
  JSON-RPC `error` → honest failure (never fake success). We mirror the posture in a
  blocking `std::net` client that fits the synchronous kernel tool path.
- **Hermes** `reference/hermes-agent-main/tools/mcp_tool.py` (L1454-1480) — the
  streamable-HTTP session: Hermes delegates to the official MCP SDK's
  `streamablehttp_client`, which manages the `Mcp-Session-Id` session id internally
  (exposed as `_get_session_id`) and re-handshakes on a dropped/expired session
  ("reconnect requested — tearing down HTTP session"). Relux has no SDK and stays
  single-POST, so we implement the same contract by hand at the HTTP layer: capture
  the `Mcp-Session-Id` response header on `initialize`, echo it on the operation's
  requests, and do one bounded re-`initialize` on a `404` session-expiry — without a
  long-lived connection or cross-call session reuse.
- **Hermes** `reference/hermes-agent-main/tools/mcp_tool.py` `_make_list_resources_handler`
  (L2434-2489) and `_make_read_resource_handler` (L2492-2548) — the resource wire shape
  and shaping. `resources/list` → `{ resources: [{ uri, name, description?, mimeType? }] }`;
  `resources/read { uri }` → `ReadResourceResult { contents: [block] }` where each block
  carries `.text` (collected) or `.blob` (binary, summarized — Hermes emits
  `[binary data, N bytes]`; we emit `[binary content omitted: <mime>]` and never decode).
  Hermes registers these as per-server utility tools (`mcp_<server>_list_resources` /
  `_read_resource`, L2842-2875) gated by the server's advertised `capabilities.resources`;
  Relux exposes the same two operations as kernel methods + read-only Prime context tools,
  and additionally **secret-redacts** the read body (Hermes does not).
- **openclaw** `reference/openclaw-main/src/tools/execution.ts`
  (`formatToolExecutorRef`) — the `mcp:<serverId>:<toolName>` executor namespace.
  We adopt `mcp:<server>` as the synthetic plugin id so MCP tools map into the
  existing `ToolDescriptor` list and route through the existing tool-invoke gates
  without colliding with real plugin ids.

Run-driven path files: `crates/relux-core/src/task.rs` (`TaskToolCall` +
`parse_task_tool_call` — the operator-named directive type/parser),
`crates/relux-kernel/src/state.rs` (`execute_local_run` routes a directive through
`call_tool` instead of echo, failing the run/task honestly on a gate refusal),
`crates/relux-kernel/src/server.rs` (`create_task` / `CreateTaskReq` accept the
optional `tool_call` directive and serialize it into the canonical input).

Relux files: `crates/relux-core/src/mcp.rs` (config + validation + discovery types +
`McpToolClassification` + `is_valid_mcp_tool_name` + injection scan, **plus the
resource types `McpResource`/`McpResourceContent` + `is_valid_mcp_resource_uri` +
resource bounds**), `crates/relux-kernel/src/mcp.rs` (loopback JSON-RPC discovery +
`call_tool` client with result shaping, **plus `list_resources` / `read_resource`
with resource shaping + secret redaction**, plus the in-memory streamable-HTTP
`Session` — `Mcp-Session-Id` capture/echo/validate + one bounded re-initialize on
`404`), `crates/relux-kernel/src/state.rs` (`register_mcp_server` /
`set_mcp_server_enabled` / `remove_mcp_server` / `mcp_servers` / `discover_mcp_tools` /
`set_mcp_tool_classification` / `clear_mcp_tool_classification` / **`list_mcp_resources`
/ `read_mcp_resource`**, the MCP branches in `resolve_tool_permission` /
`tool_needs_approval` / `execute_tool_runtime` / `matching_persistent_grant_id` /
`tool_risk_for`, **the run-bound MCP transcript events in `call_tool` (distinct
`mcp_tool_call*` kinds) + the bounded/redacted `mcp_event_result_summary` helper**,
+ the **`McpServerView` projection in `context_snapshot`**, + snapshot
persistence), `crates/relux-kernel/src/prime_tools.rs` (the read-only context tools —
**`list_mcp_servers` / `mcp_list_resources` / `mcp_read_resource`** on the
`READ_ONLY_TOOLS` allowlist + `ContextSnapshot.mcp_servers`),
`crates/relux-kernel/src/server.rs` (the registry + classification routes + **the
resource list/read routes**; invocation reuses the generic tool-invoke routes),
`crates/relux-kernel/src/store.rs` (`mcp_servers` persistence, carrying
classifications), `apps/dashboard/src/{api.ts,pages/Plugins.tsx}` (the MCP servers UI:
discover → classify → invoke / request-approval, **plus the Resources panel:
resources/list + inline read-only preview**), `apps/dashboard/src/runview.ts`
(**`phaseLabel` + `toolCallSummary` recognize the `mcp_tool_call*` run events** so a
run-bound MCP call shows in the Work run detail's Transcript + tool-call summary).

## Next MCP slice

Discovery + gated invocation + **per-operation session continuity** + **read-only MCP
resources** (`resources/list` + `resources/read`, surfaced to operators and to Prime's
read-only context loop) + **run-transcript visibility for a run-bound MCP tool call**
(the distinct `mcp_tool_call*` events above) + the **first production run path** that
routes an MCP `tools/call` through `call_tool` (the operator-named **tool-call
directive** in "Run-driven MCP tool call" above) now work end to end. Candidate next
slices, in rough order: (1) **remote transport + OAuth** (an allow-listed remote
endpoint with `mcp_oauth_manager`-style auth), gated behind an explicit operator
opt-in; (2) a **long-lived SSE / server-push subscription** (the streamable-HTTP
variant Relux still does not speak — only single-POST request/reply today);
(3) **MCP prompts** (`prompts/list` + `prompts/get`) as reusable prompt templates;
(4) a **resource-change subscription** (`notifications/resources/list_changed`) once a
server-push channel exists; (5) a **multi-step / CLI-adapter run path** that can drive
several gated MCP tools in one run (today the run-driven path executes a single
operator-named tool on the deterministic local-prime adapter; a brain freely choosing
arbitrary MCP tools mid-run stays out of scope until it routes through an
allowlisted/validated write tool and the same approval gates).
