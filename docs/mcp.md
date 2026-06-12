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

## What it does NOT do (honest limitations)

- **No stdio (command) MCP servers.** Relux never spawns arbitrary downloaded
  code. Only an operator-run loopback HTTP server is dialed.
- **No remote / `https` / SSE-subscription transport.** v1 dials no remote host.
- **Single-POST subset of streamable HTTP.** Each JSON-RPC request is its own
  `Connection: close` POST (`initialize`, then `tools/list` or `tools/call`). A
  server that requires session continuity ACROSS requests (a streamable-HTTP
  session id) is not supported — its `tools/list`/`tools/call` fails honestly
  (`McpDiscoveryFailed` / `ToolRuntimeInvocation`), never a fabricated result. A
  single SSE-framed response body (`data: {json}`) IS parsed, so simple stateless
  streamable-HTTP servers work.
- **No OAuth / auth header.** No `Authorization` is sent. (Hermes' `mcp_oauth_manager`
  is deferred.)
- **No MCP resources / prompts / sampling.** Only `tools/list` + `tools/call` are
  spoken; `resources/*`, `prompts/*`, and server-initiated sampling are not.

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
PUT    /v1/relux/mcp/servers/:id/tools/:tool/classification        { risk, approval } — set a tool's risk/approval
DELETE /v1/relux/mcp/servers/:id/tools/:tool/classification        revert a tool to the gated default
```

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
- **openclaw** `reference/openclaw-main/src/tools/execution.ts`
  (`formatToolExecutorRef`) — the `mcp:<serverId>:<toolName>` executor namespace.
  We adopt `mcp:<server>` as the synthetic plugin id so MCP tools map into the
  existing `ToolDescriptor` list and route through the existing tool-invoke gates
  without colliding with real plugin ids.

Relux files: `crates/relux-core/src/mcp.rs` (config + validation + discovery types +
`McpToolClassification` + `is_valid_mcp_tool_name` + injection scan),
`crates/relux-kernel/src/mcp.rs` (loopback JSON-RPC discovery + `call_tool` client
with result shaping), `crates/relux-kernel/src/state.rs` (`register_mcp_server` /
`set_mcp_server_enabled` / `remove_mcp_server` / `mcp_servers` / `discover_mcp_tools` /
`set_mcp_tool_classification` / `clear_mcp_tool_classification`, and the MCP branches
in `resolve_tool_permission` / `tool_needs_approval` / `execute_tool_runtime` /
`matching_persistent_grant_id` / `tool_risk_for`, + snapshot persistence),
`crates/relux-kernel/src/server.rs` (the registry + classification routes; invocation
reuses the generic tool-invoke routes), `crates/relux-kernel/src/store.rs`
(`mcp_servers` persistence, now carrying classifications),
`apps/dashboard/src/{api.ts,plugins.ts,pages/Plugins.tsx}` (the MCP servers UI:
discover → classify → invoke / request-approval).

## Next MCP slice

Discovery + gated invocation now work end to end. Candidate next slices, in rough
order: (1) **session-continuity streamable-HTTP** (carry a session id across
`initialize`/`tools/call`) so stateful MCP servers work; (2) **remote transport +
OAuth** (an allow-listed remote endpoint with `mcp_oauth_manager`-style auth),
gated behind an explicit operator opt-in; (3) **MCP resources** (`resources/list` +
`resources/read`) surfaced as a read-only Dossier source; (4) capturing an MCP call
on the **run transcript** (not just the audit log) when invoked inside a run.
