# MCP support (Relux v1 — loopback discovery)

> Scope: the **relux-* product layer** (`relux-core` / `relux-kernel` / `apps/dashboard`).
> The legacy `relix-runtime` MCP scaffold (`docs/mcp-tool.md`, `crates/relix-runtime/src/nodes/tool/mcp*.rs`)
> is a SEPARATE, older surface and is unchanged by this slice.

Spec refs: `docs/RELUX_MASTER_PLAN.md` §8.2 (ToolSet Plugins) + §18 (no
auto-running of downloaded code); `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9
("P2 — MCP tool support").

## What v1 ships

The first safe Model Context Protocol slice for Relux: an operator-curated,
**loopback-only** MCP server **registry + live tool discovery**.

An operator can:

- **Register** a loopback MCP server: `{ id, endpoint, description?, enabled?, timeout_ms? }`.
  The endpoint is validated with the SAME loopback-only rule as the plugin runtime
  (`relux_core::validate_loopback_url`): only `http://127.0.0.1|localhost|[::1]:<port>`
  with an explicit port is accepted. `https`, remote hosts, embedded credentials,
  query/fragment, and `..` paths are all rejected.
- **List** registered servers with an honest one-word status (`configured` /
  `disabled`). No secrets are ever stored or returned — only the id, endpoint,
  description, enabled flag, and per-call timeout.
- **Discover** an enabled server's tools: the kernel runs a live MCP `initialize`
  handshake followed by `tools/list` against the loopback endpoint and maps the
  result into the standard `relux_core::ToolDescriptor` shape
  (`plugin_id = "mcp:<id>"`, a derived display permission `tool:mcp-<id>:<verb>`,
  risk `Medium`, `source_kind = "Mcp"`).
- **Remove** a server registration.

## What v1 does NOT do (honest limitations)

- **MCP tool invocation is NOT wired into the agent tool-call path.** Every
  discovered tool is reported as `ToolExecutability::NotImplemented` ("discovered,
  not callable yet"). Nothing fakes an invocation. The kernel's `call_tool` /
  `invoke_tool` gates do not see MCP tools at all in v1 — the MCP surface is
  deliberately separate so a half-built call path can never bypass the
  permission / approval / grant / audit gates.
- **No stdio (command) MCP servers.** Relux never spawns arbitrary downloaded
  code. Only an operator-run loopback HTTP server is dialed.
- **No remote / `https` / SSE-subscription transport.** v1 dials no remote host.
- **Single-POST subset of streamable HTTP.** Each JSON-RPC request is its own
  `Connection: close` POST (`initialize`, then `tools/list`). A server that
  requires session continuity ACROSS requests (a streamable-HTTP session id) is
  not supported — its `tools/list` fails honestly (`McpDiscoveryFailed`), never a
  fabricated tool list. A single SSE-framed response body (`data: {json}`) IS
  parsed, so simple stateless streamable-HTTP servers work.
- **No OAuth / auth header.** v1 sends no `Authorization`. (Hermes' `mcp_oauth_manager`
  is deferred.)

Bounds (all reused from / mirrored on the loopback-tool runtime): per-call
connect/read/write timeout (clamped `100..60_000 ms`, default `10_000`), request
body cap (256 KiB), response body cap (1 MiB), at most 256 discovered tools per
server, descriptions sanitized + clamped to 600 chars and scanned for
prompt-injection patterns (advisory warning, never a block — mirrors Hermes
`_scan_mcp_description`).

## HTTP surface

```
GET    /v1/relux/mcp/servers               list registered servers (no secrets)
POST   /v1/relux/mcp/servers               { id, endpoint, description?, enabled?, timeout_ms? } (upsert by id)
DELETE /v1/relux/mcp/servers/:id           remove a server
GET    /v1/relux/mcp/servers/:id/tools     live tools/list discovery → ToolDescriptor[]
```

Honest status codes: unknown id → `404`; disabled server discovery → `409`; a
loopback transport/protocol failure → `502` (never an empty-list lie); a
non-loopback / malformed config → `400`.

## Reference-driven mapping (`docs/reference-driven-development.md`, BINDING)

Files read before implementing:

- **Hermes** `reference/hermes-agent-main/tools/mcp_tool.py` — the MCP wire shape
  (`initialize` → `tools/list` → tool `{ name, description, inputSchema }`),
  `_validate_remote_mcp_url` (http(s) + real host; we go stricter → loopback only),
  `_scan_mcp_description` + `_MCP_INJECTION_PATTERNS` (advisory injection scan we
  mirror dependency-free), per-server `timeout`/`connect_timeout` (we clamp).
  `reference/hermes-agent-main/hermes_cli/mcp_config.py` — `mcp_servers` config map
  keyed by id, `add/remove/list/test` lifecycle (we mirror register/list/remove +
  live discover).
- **Relix legacy** `crates/relix-runtime/src/nodes/tool/mcp_http.rs` — the prior
  streamable-HTTP MCP client: one POST per JSON-RPC request, `ensure_initialized`
  before `list_tools`, JSON-RPC `error` → honest failure (never fake success). We
  mirror the posture in a blocking `std::net` client that fits the synchronous
  kernel tool path.
- **openclaw** `reference/openclaw-main/src/tools/execution.ts`
  (`formatToolExecutorRef`) — the `mcp:<serverId>:<toolName>` executor namespace.
  We adopt `mcp:<server>` as the synthetic plugin id so MCP tools map into the
  existing `ToolDescriptor` list without colliding with real plugin ids.

Relux files: `crates/relux-core/src/mcp.rs` (config + validation + discovery
types + injection scan), `crates/relux-kernel/src/mcp.rs` (loopback JSON-RPC
discovery client), `crates/relux-kernel/src/state.rs` (`register_mcp_server` /
`set_mcp_server_enabled` / `remove_mcp_server` / `mcp_servers` / `discover_mcp_tools`,
+ snapshot persistence), `crates/relux-kernel/src/server.rs` (the four routes),
`crates/relux-kernel/src/store.rs` (`mcp_servers` persistence),
`apps/dashboard/src/{api.ts,plugins.ts,pages/Plugins.tsx}` (the MCP servers UI).

## Next MCP slice

Route `tools/call` through the existing tool-invocation gates: adopt the derived
`tool:mcp-<server>:<verb>` permission as enforced (not display-only), surface MCP
tools through `call_tool` / `invoke_tool` with the per-call / persistent-grant +
approval flow intact, let the operator classify each tool's real risk, and capture
the call on the audit log + run transcript. Keep bounds + redaction. Only after
that lands should an MCP tool ever read as anything other than `not_implemented`.
