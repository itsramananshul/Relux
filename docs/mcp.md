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

## Managed stdio MCP servers (governed local command transport)

Many real MCP servers are **stdio commands** (`npx @modelcontextprotocol/server-…`,
`python -m mcp_server`), not loopback HTTP endpoints. Relux now registers and runs
these as a **governed managed-stdio transport** — a second safe transport alongside
loopback HTTP — so an operator no longer has to stand up a loopback HTTP shim by hand.
It stays safe by construction and **never runs downloaded code on import**.

Reference (`docs/reference-driven-development.md`, BINDING):
`reference/hermes-agent-main/hermes_cli/mcp_config.py` (`cmd_mcp_add` /
`_probe_single_server`) — a server is `{"url"}` HTTP or `{"command","args","env"}`
stdio, spawned via the MCP SDK and probed by connecting → `tools/list` → disconnect;
`crates/relix-runtime/src/nodes/tool/mcp_stdio.rs` — the prior async stdio MCP client
(spawn with `kill_on_drop`, `initialize` + `notifications/initialized`, drain
notifications until a matching-id response, map every failure to an honest error).
Relux ports that posture to a **blocking** `std::process` client
(`crates/relux-kernel/src/mcp_stdio.rs`) that fits the synchronous kernel tool path,
and goes **stricter** than Hermes.

- **Two transports in one registry.** `relux_core::McpTransport` now has
  `HttpLoopback` (the original loopback HTTP endpoint) and `ManagedStdio` (a local
  command). `McpServerConfig` carries `command: Option<String>` + `args: Vec<String>`
  for the stdio transport (and `endpoint` for HTTP); both serialize/deserialize, and
  the config still **stores no secret**. Registered via the SAME
  `POST /v1/relux/mcp/servers` route — `{ "id", "transport":"managed_stdio",
  "command":"npx", "args":[…], "description"?, "enabled"?, "timeout_ms"? }` (a present
  `command` also selects stdio) — and the SAME list/enable/remove/classify surfaces.
- **Registration is explicit + operator-confirmed; it never spawns.** Registering a
  stdio server only records the (validated) `command` + `args`. The command is spawned
  **only** on a later operator-driven **Discover** (`GET …/tools`) or a **gated
  invocation** — never on registration and never on plugin import.
- **argv only — never a shell.** The command + args are validated by
  `relux_core::validate_stdio_command` and passed straight to `std::process::Command`
  as `argv`. The command must be a single bounded program token with **no shell
  metacharacter** (`; | & $ \` < > ( ) { } [ ] * ? ! # ' "`) and no control character
  (a space IS allowed, so a full path like `C:\Program Files\nodejs\node.exe` works —
  it is one argv token, never split). Each arg is bounded, control-character free, and
  may otherwise contain anything (flags, `=`, JSON) because it is one `argv` element,
  never shell-expanded. The arg **count** is bounded. There is **no shell-injection
  surface**.
- **Governed env + `cwd`, no bypass flags.** The child inherits the parent environment
  plus the operator-configured `env` — each value a **secret reference** resolved from
  the local secret store at spawn (never a literal value in the config), see "Local
  secrets & environment" below — and runs in the optional `cwd` (validated INSIDE the
  safe workspace root). Relux **never injects** a bypass/danger flag: a small denylist
  (`--dangerously-skip-permissions`, `--dangerously-bypass-approvals-and-sandbox`,
  `--yolo`) **refuses** an operator-supplied bypass flag fail-closed — consistent with
  the adapter governance (`crate::adapter` never passes one either). A missing secret /
  bad `cwd` is a clean **failed status naming the missing key** (never a value), not a
  spawn.
- **Two process modes: spawn-per-operation OR an operator-managed process.** The
  original transport **spawns its own child, runs one logical operation (`initialize`
  → `tools/list` / `tools/call`), and kills + reaps it** (`StdioChild::drop`) — exactly
  like the HTTP client opens a fresh `Connection: close` POST per operation. That stays
  the safe **fallback** (used when no managed process is running). On top of it, an
  operator can **Start** a managed server so a single `initialize`d process stays warm
  and Discover / invocation **reuse** it (no per-call spawn + handshake) — see
  "Managed-stdio process lifecycle" below. Either way the process is bounded, reaped,
  and never runs without an explicit operator action.
- **Timeout + size + redaction bounds.** Each request is bounded by the per-call
  timeout (the child is killed on expiry); each stdout response line is size-capped
  (4 MiB); the child's **stderr is drained into a bounded, secret-redacted tail**
  (`relux_core::redact_secrets`, ≤20 lines) folded into the failure message so an
  operator can see *why* a spawn/handshake/call failed. The result of a `tools/list`
  / `tools/call` reuses the **same** bounded, sanitized, secret-redacted shapers as
  the HTTP client (`crate::mcp::parse_tools_list` / `shape_tool_call_result`) — the
  transport differs, the result handling does not. A `tools/call` `isError` is an
  honest runtime failure, never a fabricated success; the **raw JSON-RPC envelope is
  never returned**.
- **Same gates, same `mcp:<server>` namespace.** A discovered stdio tool maps into the
  identical `ToolDescriptor` shape (`plugin_id = "mcp:<server>"`, permission
  `tool:mcp-<server>:<verb>`, fail-closed Medium+Required until classified) and is
  **invoked through the unchanged `call_tool` / `invoke_tool` / per-call-approval /
  persistent-grant path**. Registering or running a stdio server **does not
  auto-approve its tools**. The plan-proposal grounding (`discover_proposal_mcp_catalog`)
  dispatches on transport too, so an `mcp:<server>/<tool>` step grounds against a live
  stdio `tools/list` the same way it does an HTTP one.
- **JSON-RPC subset (tools + read-only resources + read-only prompts).** The stdio
  bridge speaks `initialize`, `notifications/initialized`, `tools/list`, `tools/call`,
  the two **read-only** MCP **resource** methods `resources/list` / `resources/read`
  (managed stdio resources v1, below), and — as of **managed stdio prompts v1** — the
  two **read-only** MCP **prompt** methods `prompts/list` / `prompts/get` (see
  "Managed-stdio MCP prompts (v1)" below), and — as of **MCP sampling v1** — handles a
  SERVER-initiated `sampling/createMessage` request as a **gated, default-deny** safe
  subset (see "MCP sampling (v1)" below). It still does **not** speak resource
  **subscriptions** (a streaming method); that remains out of scope (see "Remaining gaps").

### Plugin MCP hint → managed-stdio prefill (advisory, never auto-run)

When an imported plugin's MCP config names a stdio `{command, args}`,
`crate::mcp_proposal::propose_mcp_registration` now **pre-fills a reviewable
managed-stdio registration draft** (`suggested_transport = "managed_stdio"`,
`detected_command`, `detected_args`) instead of treating the command as display-only
text. It still **executes nothing** on import — it only reads the same bounded
metadata files the hint scan reads — and registration still flows through the
unchanged `POST /v1/relux/mcp/servers` route + `validate_stdio_command`. The dashboard
form seeds from the draft (`apps/dashboard/src/plugins.ts` `mcpDraftFromProposal`), the
operator **reviews + confirms** the command/args (the same fail-closed pre-check
`validateMcpRegisterDraft` mirrors the kernel), and the command is spawned only on a
later operator-driven Discover/invoke. A detected non-loopback / missing `url` still
forces manual entry; nothing is auto-registered or auto-run.

### Managed-stdio process lifecycle (operator Start / Stop / Restart + reuse)

A registered managed-stdio server is **registered** (a config row) independently of
whether its **process is running**. Beyond the per-operation spawn, an operator can run
a single long-lived process and reuse it — the real lifecycle a serious product needs
(Hermes keeps the MCP client connected between `list_tools` / `call_tool`; its
`_probe_single_server` connects → lists → disconnects only for a one-shot probe —
`reference/hermes-agent-main/hermes_cli/mcp_config.py`). It stays safe by construction.

- **A managed pool, outside the snapshot.** `crate::mcp_stdio::pool()` is a
  process-global pool (`ManagedPool`) keyed by server id; it lives **outside** the
  serializable `KernelState` (a live OS process is not snapshot state). The kernel's
  registry stays the source of truth for *what* is registered; the pool owns *whether
  it is running*. The kernel drives it (`start_mcp_stdio_server` / `stop…` / `restart…`
  / `mcp_stdio_status` / `mcp_stdio_statuses`), each validated against the registry
  (the server must exist, be a managed-stdio transport, and — to start — be enabled),
  audited (`mcp:server_start` / `_stop` / `_restart`), and honest.
- **Explicit lifecycle, never auto-started.** A process spawns only on an explicit
  **Start** (or, when no managed process is running, a per-operation Discover/invoke
  via the fallback). Start runs the spawn + `initialize` once; **Stop** kills + reaps
  the child; **Restart** stops then starts a fresh one; removing the registration stops
  + reaps any process so a removed server never leaves a daemon behind; and a managed
  process is always killed + reaped on stop / restart / drop / process shutdown.
- **Reuse for `tools/list` + `tools/call`.** While a managed process is **running**,
  Discover (`tools/list`) and gated invocation (`tools/call`) **reuse it** — one
  `initialize`d process, monotonically increasing JSON-RPC request ids, responses
  matched to their request id (a stale reply or a notification is drained, never
  confused for the answer), per-call timeout, and process-death detection. When no
  process is running, the same operations fall back to the safe spawn-per-operation
  transport — so Discover/invoke never silently fail just because nothing was started.
- **Status is honest.** Each managed server reports `stopped` / `starting` / `running`
  / `failed`, the `pid` (safe to show) and start time when up, the redacted `last_error`
  on a failure (spawn failed, the process died, a fatal transport error), the
  `tools_count` from the last live `tools/list`, and a bounded, **secret-redacted**
  stderr **log tail** (`relux_core::ManagedStdioStatus`). A start that fails to spawn is
  a `failed` status with the reason — **never** a fabricated `running`. A running
  process that dies (or a call that hits a fatal transport error / timeout) is torn down
  and marked `failed` with the reason — **never** a fabricated success. An *application*
  error (a JSON-RPC `error`, or a `tools/call` `isError`) leaves the process healthy and
  reusable; it does not kill the daemon.
- **Bounded process + log memory.** One process per registered stdio server; the stderr
  tail is bounded (`relux_core::MAX_MANAGED_STDIO_LOG_LINES`, redacted); status fields
  are cheap atomics read without blocking a live request (so `starting` is observable
  mid-spawn). Same safety contract as the per-operation transport: **argv only (never a
  shell), no plaintext secret stored (only references), `cwd` confined to the safe
  workspace root, no bypass/danger flag.** The resolved env values are handed straight
  to the spawn and are **never** retained on the entry, the status, or the log tail.
- **Routes.** `GET /v1/relux/mcp/servers/status` (all stdio servers),
  `GET …/:id/status`, `POST …/:id/start`, `POST …/:id/stop`, `POST …/:id/restart`. A
  lifecycle action against an HTTP-loopback server is a `400` (it has no process
  lifecycle — it is an endpoint the operator runs); an unknown server is `404`; a
  disabled server refused to start is `409`.

### Managed-stdio MCP resources (v1) — `resources/list` + `resources/read`

MCP **resources** are a **read-only context surface** (files, records, docs an agent can
read) — distinct from tools (which act). They were originally an HTTP-only surface; a
managed-stdio server is **stdio commands**, so to feel like a real MCP product surface
the bridge now speaks `resources/list` and `resources/read` over stdio too. It stays
**read-only by construction** — no `tools/call`, no mutation, no new authority.

Reference (`docs/reference-driven-development.md`, BINDING):
`reference/hermes-agent-main/tools/mcp_tool.py` — `_make_list_resources_handler`
collects `{ uri, name, title?, mimeType?, description? }`; `_make_read_resource_handler`
concatenates the `contents` text blocks and summarizes a binary (`blob`) block. Relux
already ports that shaping for the HTTP client (`crate::mcp::parse_resources_list` /
`shape_resource_read_result`); managed stdio reuses the **same** shapers, so the
transport differs and the result handling does not.

- **Two read-only methods, same two process modes.** The managed-stdio client
  (`crates/relux-kernel/src/mcp_stdio.rs`) gains `list_resources` / `read_resource`
  (spawn-per-operation: spawn → `initialize` → the request → reap) and the pool gains
  `ManagedPool::list_resources` / `read_resource` (reuse the operator-started
  `initialize`d process). The kernel dispatches on transport (`mcp_list_resources` /
  `mcp_read_resource` in `state.rs`): a running managed process is **reused**; otherwise
  it falls back to the safe spawn-per-operation read (resolving `env` secrets +
  validating `cwd` first, exactly like the tools path) — so a stdio resource read never
  silently fails just because nothing was started.
- **Identical shaping, bounds, and redaction as HTTP.** `resources/list` reuses
  `parse_resources_list` (bounded to `MAX_MCP_RESOURCES`, every string sanitized +
  clamped, a URI-less entry skipped rather than fatal). `resources/read` reuses
  `shape_resource_read_result`: text content blocks are concatenated, sanitized,
  **secret-redacted** (`relux_core::redact_secrets` — a credential embedded in a
  resource body never leaks verbatim), and clamped to `MAX_MCP_RESOURCE_TEXT_CHARS`; a
  binary (`blob`) block is summarized with an honest `[binary content omitted: <mime>]`
  marker and its bytes are **never decoded or returned**. The **raw JSON-RPC envelope is
  never returned**. The URI is validated fail-closed (`is_valid_mcp_resource_uri`) before
  any spawn, even over stdio.
- **Same surfaces, no new route or authority.** The existing operator API
  (`GET /v1/relux/mcp/servers/:id/resources` and
  `…/resources/read?uri=…`) and the read-only Prime context tools
  (`mcp_list_resources` / `mcp_read_resource`) now dispatch on transport, so they work
  for a managed-stdio server with no new route. The dashboard's **Resources** action +
  panel (previously hidden for a stdio server) is shown for both transports; it runs the
  same read-only `resources/list` + inline `resources/read` preview. Honest by
  construction: an unknown server → 404, a disabled one → 409, an invalid URI → 400, a
  transport/protocol failure → 502 (never a fabricated list/body).
- **Test fixture.** The pure-Rust MCP stdio fixture
  (`crates/relux-kernel/src/bin/relux_mcp_test_server.rs`) now advertises a text resource
  (`mem://notes`, embedding an obvious fake secret to prove redaction) and a binary one
  (`mem://image`, a `blob`), exercised end to end (spawn-per-op, pool reuse, and through
  the kernel registry) in `crates/relux-kernel/tests/mcp_stdio.rs`.

### MCP prompts (v1) — `prompts/list` + `prompts/get` (HTTP + managed stdio)

MCP **prompts** are a **read-only template surface**: a server advertises named,
parameterizable prompt templates via `prompts/list`, and a client materializes one (with
arguments) into chat messages via `prompts/get`. They are a THIRD read-only surface
alongside tools (which act) and resources (read-only data). They stay **read-only by
construction** — listing or getting a prompt performs no action and mutates nothing, and
crucially a `prompts/get` returns **template text Relux shows as context, NOT a turn Relux
executes**. There is **no new authority**: no `tools/call`, no mutation, no auto-run.

Reference (`docs/reference-driven-development.md`, BINDING):
`reference/hermes-agent-main/tools/mcp_tool.py` — `_make_list_prompts_handler` (L2552-2612)
collects `{ name, description?, arguments:[{name, description?, required?}] }`;
`_make_get_prompt_handler` (L2615-2681) maps `GetPromptResult.messages` to `{ role,
content }`, taking a content block's `.text` (or a bare string) as the message text. Relux
ports that shaping for **both** transports: the HTTP client (`crate::mcp::list_prompts` /
`get_prompt`, with `parse_prompts_list` / `shape_get_prompt_result`) and the managed-stdio
client (`crate::mcp_stdio::list_prompts` / `get_prompt` + `ManagedPool` reuse), which reuse
the **same** shapers — the transport differs, the result handling does not.

- **Two read-only methods, both transports, same two process modes.** `prompts/list` and
  `prompts/get` work over loopback HTTP (dial the endpoint, streamable-HTTP session) and
  managed stdio (reuse the operator-started `initialize`d process, else fall back to the
  safe spawn-per-operation read — resolving `env` secrets + validating `cwd` first, exactly
  like the tools/resources paths). The kernel dispatches on transport
  (`mcp_list_prompts` / `mcp_get_prompt` in `state.rs`).
- **Bounded, sanitized, redacted shaping.** `prompts/list` is bounded to
  `MAX_MCP_PROMPTS`; each prompt's name/description and its declared `arguments` (bounded
  to `MAX_MCP_PROMPT_ARGS`, each `{name, description, required}`) are sanitized + clamped; a
  nameless entry is skipped rather than fatal. `prompts/get` maps each message (bounded to
  `MAX_MCP_PROMPT_MESSAGES`) to `{ role, content }`: a `{ text }` (or `{ type:"text", text
  }`) block is taken verbatim, a bare string is used as-is, an array of blocks is joined
  (a non-text block summarized as `[non-text content: <type>]`), and the content is then
  sanitized, **secret-redacted** (`relux_core::redact_secrets` — a credential embedded in a
  prompt template never leaks verbatim), and clamped to `MAX_MCP_PROMPT_MESSAGE_CHARS`. The
  **raw JSON-RPC envelope is never returned**. The prompt name is validated fail-closed
  (`is_valid_mcp_prompt_name` — non-empty, bounded, control-char free) before any spawn/dial.
- **Surfaces (operator API + read-only Prime context tools).** New operator routes
  `GET /v1/relux/mcp/servers/:id/prompts` (list) and
  `POST /v1/relux/mcp/servers/:id/prompts/get { name, arguments? }` (materialize one — POST
  carries the structured args object but holds only a **read lock**; a `prompts/get`
  mutates nothing). Honest by construction: an unknown server → 404, a disabled one → 409,
  an invalid/empty name → 400, a transport/protocol failure → 502 (never a fabricated
  list/body). The read-only Prime context tools `mcp_list_prompts` / `mcp_get_prompt`
  (`prime_tools.rs`) dispatch on transport off-lock; `mcp_get_prompt` returns the template
  text as an **observation** (context the brain reads) — it is **never** auto-run as a user
  turn. The dashboard's **Prompts** action + panel (`apps/dashboard/src/pages/Plugins.tsx`
  `McpPromptsPanel` / `McpPromptRow`) is shown for both transports; it runs the read-only
  `prompts/list` and an inline `prompts/get` with a minimal arguments form.
- **Test fixture.** The pure-Rust MCP stdio fixture now advertises a `greet` prompt
  (a required `who` argument materialized into a message) and a `leaky` prompt (its message
  embeds an obvious fake secret to prove redaction), exercised end to end (spawn-per-op,
  pool reuse, through the kernel registry, and the Prime snapshot tool) in
  `crates/relux-kernel/tests/mcp_stdio.rs` + the HTTP shaper/route tests in
  `crates/relux-kernel/src/mcp.rs` + `state.rs` + `server.rs`.

### MCP sampling (v1) — gated, default-deny server-initiated LLM calls

MCP **sampling** (`sampling/createMessage`) **inverts the trust direction** of every
other MCP method. For tools / resources / prompts, Relux drives the server. For sampling,
the server — mid-operation, over a managed-stdio **session** — sends a
`sampling/createMessage` REQUEST *back to Relux* and blocks until Relux runs its OWN
configured LLM and returns the completion. That is genuinely dangerous: a
hostile/compromised server could drive Relux's model, burn the operator's provider
budget, or try to exfiltrate a secret. So Relux ships a **gated safe subset**, fail-closed
by construction — never a faked capability, never a silent hang.

Reference (`docs/reference-driven-development.md`, BINDING):
`reference/hermes-agent-main/tools/mcp_tool.py` +
`reference/hermes-agent-main/website/docs/user-guide/features/mcp.md` (Hermes exposes a
per-server sampling handler over its MCP client). Relux ports the *shape* (a client-side
handler for a server-initiated request) but goes **stricter**: default-deny,
capability-gated, provider-key-isolated, bounded, redacted, audited, and limited to a
single text completion (no tool calls, no task/run mutation).

- **Managed-stdio sessions only.** Sampling is served **only** from an operator-started
  **managed-stdio process** (the warm `initialize`d session) — the one transport with a
  persistent server→client channel. The spawn-per-operation fallback is **not** a session:
  it runs with sampling off and cleanly refuses any server-initiated request. Enabling
  sampling on an **HTTP-loopback** server is rejected fail-closed by
  `relux_core::validate_mcp_server_config` (a stateless POST-per-op endpoint has no
  session to carry the request).
- **Default deny + capability gating.** `McpServerConfig.sampling.enabled` is **`false` by
  default**. The `sampling` capability is **advertised in the `initialize` handshake ONLY
  when the session is serviceable** — the operator enabled it AND a Prime/AI provider is
  configured (`SamplingContext::serviceable`). So a spec-compliant server will not even
  ask when sampling is off; and if a non-compliant one asks anyway, the request is still
  cleanly **refused** (it is never run).
- **Clean refusal, never a hang (the real gap closed).** Before this slice, the stdio
  client silently drained any server→client message that was not the response it was
  waiting for — so a `sampling/createMessage` request hung the server until the per-call
  timeout killed it. Now the pump (`StdioChild::request` in
  `crates/relux-kernel/src/mcp_stdio.rs`) detects a server-initiated request (a message
  carrying a `method`, checked **before** the id-match so a server id that collides
  numerically with ours is never confused for our response) and dispatches it to
  `crate::mcp_sampling::handle_inbound_request`: a gated `sampling/createMessage`, or a
  clean **`-32601`** for any other server-initiated method. The response is written back
  immediately and the client keeps waiting for ITS response — the server degrades instead
  of hanging.
- **The decision (fail-closed, `crate::mcp_sampling`).** `sampling/createMessage` is
  refused with an honest JSON-RPC error when: sampling is **disabled** by policy
  (`-32001`), **enabled but no provider** is configured (`-32002`), or the request is
  **malformed / over the input bounds** (`-32003`). Only an enabled + provider-backed +
  well-formed request is served; a provider failure is an honest `-32010` (never a
  fabricated completion).
- **The provider key NEVER reaches the server.** An allowed completion is produced by a
  synchronous `Sampler` the kernel builds from the resolved `AiConfig`
  (`crate::ai::build_sampling_sampler`) — the OpenRouter key lives only on the in-memory
  config (sourced by **secret reference**, see "Prime brain provider key by reference")
  and travels solely in the `Authorization` header inside `request_completion`. The stdio
  pump is synchronous and may already be inside the server's async runtime, so the
  provider call runs on a **dedicated OS thread with its own current-thread Tokio runtime**
  (a fresh thread is never a Tokio worker, so `block_on` there is safe). Only the
  completion **text** — clamped and `relux_core::redact_secrets`-redacted — is returned to
  the server. A credential the model echoes is masked **before** it leaves for the
  (possibly hostile) server.
- **Bounded input + output.** Input is bounded to `MAX_MCP_SAMPLING_MESSAGES` (32)
  messages, `MAX_MCP_SAMPLING_MESSAGE_CHARS` (8 000) per message, and a total of
  `MAX_MCP_SAMPLING_INPUT_CHARS` (16 000) chars (the system prompt is bounded too); the
  server-requested `maxTokens` is clamped to `MAX_MCP_SAMPLING_MAX_TOKENS` (1 024). Output
  is clamped to `MAX_MCP_SAMPLING_OUTPUT_CHARS` (8 000) and redacted. **No tool calls** are
  possible (it is a single text completion — no tools are exposed to that model) and **no
  task/run mutation** can occur.
- **Audited (secret-free).** Every request records a `relux_core::McpSamplingAuditRecord`
  on a process-global tail (`crate::mcp_sampling::audit_tail`, outside `KernelState` like
  the managed pool): the server id, the decision (`allowed` / `denied_policy` /
  `denied_no_provider` / `bounds_error` / `provider_error`), a redacted reason, the input/
  output **char counts**, and the model id — **never** the request messages, the
  completion text, or any key.
- **Policy is per-process (Start/Restart to apply).** Like the env model, the sampling
  context is attached to the session at **Start/Restart** (the capability is advertised at
  handshake time). Enabling/disabling sampling takes effect on the next Start/Restart of
  the managed process.
- **Surfaces.** `PUT/PATCH /v1/relux/mcp/servers/:id/sampling { "enabled": bool }` sets
  the per-server policy (fail-closed: 400 on an HTTP server, 404 unknown); the server
  listing carries `sampling_enabled`; `GET /v1/relux/mcp/sampling/audit` returns the
  secret-free audit tail. The dashboard's managed-stdio **Process** card has a **Sampling**
  row (`apps/dashboard/src/pages/Plugins.tsx` `ManagedStdioControls`) showing the policy
  badge, an Enable/Disable toggle, and the honest limits + provider/restart note.
- **Why per-request UI approval is future (honest).** The MCP `sampling/createMessage`
  request is **synchronous** — the server blocks the in-flight operation until the client
  answers. Pausing for an interactive operator approval mid-request would hold the server
  (and the kernel call) blocked indefinitely, risking a deadlock and a timeout teardown of
  the warm process. So v1 is **policy-based allow/deny** (a standing per-server toggle),
  not a per-request prompt. A future async-approval design (answer the request with a
  "pending" and resolve out of band) can layer on top without changing the gates.
- **Test fixture + tests.** The pure-Rust MCP stdio fixture
  (`crates/relux-kernel/src/bin/relux_mcp_test_server.rs`) advertises a `sample_probe`
  tool that, during its `tools/call`, sends a SERVER→client `sampling/createMessage`
  request (id `9001`, distinct from the client's monotonic ids) and returns whatever the
  client answered. End-to-end tests in `crates/relux-kernel/tests/mcp_stdio.rs` prove
  denied-by-default, allowed-with-a-test-provider (output redacted + clamped + audited),
  missing-provider clean refusal, the HTTP-rejection + policy-persistence, plus the pure
  decision/bounds/redaction unit tests in `crate::mcp_sampling`.

### Local secrets & environment (API keys for managed-stdio MCP servers)

Real MCP servers (and future adapters) need credentials — an `OPENAI_API_KEY`, a GitHub
token, a service URL. Relux now offers a **safe, local secret store + secret-referenced
`env` + a confined `cwd`** so an operator can supply those **without hard-coding them and
without ever exposing them**. The config never stores a plaintext value; the dashboard
never receives one back.

Spec refs: `docs/RELUX_MASTER_PLAN.md` §17.5 (permissions/safety) + §8.2 (ToolSet/adapter
plugins). Reference (`docs/reference-driven-development.md`, BINDING):
`reference/hermes-agent-main/hermes_cli/mcp_config.py` — a stdio server is
`{"command","args","env"}`; a per-server API key is stored in a **separate** `~/.hermes/.env`
(`save_env_value`, keyed `MCP_<NAME>_API_KEY`) and **referenced** from the config via a
`${ENV}` ref, and `cmd_mcp_test` only ever prints a **masked** value
(`resolved[:4] + "***" + resolved[-4:]`, L553-560) — never the raw secret;
`crates/relix-web-bridge/src/secrets.rs` + `os_secure.rs` — a separate
permission-restricted file (mode `0600` / `icacls`), atomic write, no-plaintext-return,
tail-redacted preview. Relux ports that posture to the relux layer.

- **The secret store (`relux-kernel::secret_store`).** A local, file-backed store
  (`secrets.json` next to the control-plane DB, `RELUX_SECRETS_FILE` to override),
  **hardened to owner-only permissions** (POSIX `0600`; Windows `icacls` inheritance
  stripped + current-user-only). It lives **outside** the kernel snapshot (like the
  managed pool) so a plaintext credential never lands in the control-plane DB, the API,
  or an export. An operator can:
  - **Set** a named secret (`PUT /v1/relux/secrets/:name { "value": … }`) — the value is
    **write-only**: the response carries only a **redacted** `SecretStatus`
    (name + set time + a `…cdef` tail preview), never the value.
  - **List** secrets (`GET /v1/relux/secrets`) — redacted statuses only.
  - **Delete** a secret (`DELETE /v1/relux/secrets/:name`) — idempotent.
  Names are bounded + charset-restricted (`is_valid_secret_name`), values are bounded
  (`MAX_SECRET_VALUE_BYTES`, 16 KiB), and the store is count-capped (`MAX_SECRETS`, 256).
  The **only** method that returns plaintext is `resolve(name)`, called solely at
  managed-stdio spawn / Prime-brain request time to seed the child env / the auth header —
  never logged, stored back, or returned over HTTP.

- **Encrypted at rest (Windows DPAPI), with an honest fallback.** Each secret carries a
  per-value **scheme marker** so the at-rest encoding is explicit and migratable
  (`relux-kernel::secret_cipher`):
  - **Windows → `dpapi_current_user`.** The value is sealed with **DPAPI, CurrentUser
    scope** (`CryptProtectData`, driven through PowerShell's
    `System.Security.Cryptography.ProtectedData` — the same shell-out posture as
    `os_secure`'s `icacls`, so the kernel stays free of `unsafe` and a heavyweight
    `windows` crate). Only the same Windows user on the same machine can unseal it; the
    file alone is useless to a thief / backup / other admin. The plaintext **never rides
    an argv** — it travels base64 over the child's stdin/stdout pipes only. The stored
    value is `base64(CryptProtectData blob)`.
  - **Other OSes / DPAPI unavailable → `plaintext_file_v1`.** No OS-keychain integration
    yet, so the value is stored verbatim and protected only by the owner-only file
    permissions — **honestly marked** so the dashboard shows "plaintext (file-locked)"
    rather than implying encryption. This is also the **fail-safe fallback** on Windows
    when DPAPI is unavailable: a sealing failure stores plaintext (never loses the secret)
    and records the honest scheme.
  - **Reads dispatch on the stored scheme**, so a mixed-scheme file (mid-migration) reads
    correctly, and a value sealed on one host that another can't unseal (e.g. a DPAPI file
    copied to Linux) **fails closed** with a clean, value-free error naming the secret +
    scheme — never a silent wrong answer, never the value.
  - **Migration is automatic + safe.** On load (`open`/`attach`), any legacy
    `plaintext_file_v1` entry is **re-sealed** to the active encrypting scheme and the file
    rewritten — so an existing plaintext `secrets.json` upgrades to DPAPI on the next
    Windows boot. A re-seal failure leaves that entry exactly as-is (never dropped). A
    plaintext host leaves a plaintext file untouched. Setting a secret again also rewrites
    it under the current scheme (manual rotation = re-set).
  - **Status exposes the scheme, never the value.** `SecretStatus` now carries `scheme`
    alongside `name` / `set_at` / `preview`, so the operator can see which secrets are
    encrypted at rest. The redacted **preview is precomputed at set time** (and derived
    live only for a legacy plaintext entry), so `list`/`status` **never decrypt**.

- **Secret-referenced `env` on a managed-stdio server.** `McpServerConfig.env` is a map
  keyed by the **env-var NAME** the child receives, valued by a **secret reference**
  `{ "secret": "<name>" }` (`relux_core::McpEnvRef`) — never a literal value, so the
  config (and the snapshot, and every API response) **stores no plaintext**. Wire shape:
  `"env": { "OPENAI_API_KEY": { "secret": "openrouter_api_key" } }`. Env-var names are
  validated POSIX-style (`is_valid_env_var_name`, mirroring Hermes' `_ENV_VAR_NAME_RE`),
  the referenced secret name is validated, and the count is bounded
  (`MAX_MCP_ENV_VARS`, 64). The referenced secret **need not exist at registration** —
  it is resolved (naming the missing key, never a value, on failure) at **spawn**.

- **Resolution at spawn (off-snapshot, never serialized).** At a managed-stdio **Start**,
  **Restart**, or spawn-per-operation **Discover/invoke**, the kernel resolves each
  `env` ref to its plaintext via the secret store and hands a `(name, value)` list
  **straight to `std::process::Command::env`** (`crate::mcp_stdio::build_command`). The
  resolved values are **never** stored on the kernel, the managed-pool entry, the status,
  or the redacted stderr/log tail. A **missing** secret produces a clean **`failed`
  status** whose `last_error` names the missing **secret + env-var key** (never a value);
  the spawn does **not** happen. (Process stderr is still secret-redacted via
  `relux_core::redact_secrets` as defense in depth.)

- **Confined optional `cwd`.** `McpServerConfig.cwd` (a path string; no secret) sets the
  child's working directory. It is validated **fail-closed**
  (`relux-kernel::validate_managed_cwd`): the shape must pass
  `relux_core::validate_stdio_cwd_shape` (non-empty, bounded, no control char, **no `..`
  traversal**); the path (relative → resolved against the safe root, absolute → as-is)
  must **exist**, **canonicalize**, be a **directory**, and **canonicalize INSIDE** the
  configured **safe MCP workspace root** (`dev-data/relux/mcp-workspaces` by default,
  `RELUX_MCP_WORKSPACE_ROOT` to override) — `canonicalize` resolves symlinks before the
  containment check, so a symlink that points outside the root is rejected. A `cwd` set
  with no configured root is refused. This is the "explicitly safe configured workspace
  root" model — a managed-stdio `cwd` can only ever resolve inside that one tree.

- **Security model (binding).** The config layer carries **only secret references + a
  path** — never a value. Plaintext lives **only** in the owner-only `secrets.json` and,
  transiently, in the spawned child's environment. No surface returns a secret value: the
  list/set/delete API, the server listing, the managed-process status, the run
  transcript, and the audit log all carry redacted previews / names only. The same
  argv-only, no-shell, no-bypass-flag guarantees of the managed transport are unchanged;
  protected/bundled plugins are untouched; nothing auto-starts or auto-runs on
  registration.

- **Limitations (honest).** The store is **local-only**. At rest it is **encrypted on
  Windows (DPAPI, CurrentUser)** and **permission-hardened plaintext elsewhere** (no
  macOS/Linux keychain integration yet — the `plaintext_file_v1` scheme is honestly
  surfaced, not silently implied to be encrypted). DPAPI is **CurrentUser-scoped**: it
  protects against another user / an offline disk image, not against code already running
  as the same user (which is the trust level that legitimately needs the key at spawn
  time). Secret **rotation** is manual (set the same name again, which re-seals it).
  `env` resolution is **per-process** — a running managed process keeps the env it was
  started with; change a secret and **Restart** to pick it up. The **Prime brain
  provider** (OpenRouter) now consumes the store by reference (see "Prime brain provider
  key by reference" below); the CLI adapters (Claude/Codex) authenticate through their
  own local CLI login and need no key here.

### Prime brain provider key by reference (OpenRouter)

Spec: `docs/RELUX_MASTER_PLAN.md` §8.1 (adapter plugins) + "Optional LLM-backed Prime".

The OpenRouter brain (the one HTTP/API provider that takes a key) sources its API key
by **secret reference**, exactly like a managed-stdio server's `env`: the key value lives
**only** in the write-only secret store, and the AI config carries only the secret's
**name**.

- **Stored as a reference, never plaintext.** `relux_kernel::StoredAiConfig` has an
  `api_key_secret` field (the secret NAME) that is **mutually exclusive** with the legacy
  plaintext `api_key` — `write_stored_config` clears one when the other is set, so there
  is a single source of truth. The dashboard writes **only** `api_key_secret`; it never
  sends or stores a plaintext key. (`api_key` remains only for the legacy env/CLI path.)
- **Resolved at request time, off-snapshot.** `AiConfig::resolve` (and the pure,
  testable `AiConfig::resolve_with`) resolves the referenced secret to plaintext through
  the **same** `secret_store()` at the moment Prime needs it, holds it privately on the
  in-memory `AiConfig`, and uses it only in the `Authorization` header
  (`request_completion`). The plaintext is never serialized, logged, or returned.
- **Missing secret fails cleanly.** If `api_key_secret` names a secret that is not set,
  resolution yields **no usable key**, `AiStatus { configured: false, secret_missing:
  true }`, and a `reason` that **names the missing secret** and what to do — Prime stays
  deterministic instead of silently failing. No raw key ever appears.
- **Status is key-free.** `GET /v1/relux/ai/status` returns the brain, `configured`,
  `secret_missing`, and the referenced `api_key_secret` **name** (never the value).
- **Set it from the dashboard.** The Prime Brain panel → OpenRouter → **Prime AI
  settings** (`PrimeAiSettings`) offers a secret **picker** (existing secrets, by name +
  redacted preview) plus an inline **"Create a new secret…"** (write-only value) that
  stores the key in the secret store and references it in one step. "Clear key reference"
  drops the reference without deleting the stored secret.

### Operating it (dashboard)

The Plugins page has a **Secrets & environment** section (`SecretsSection`) where the
operator adds named secrets — the value field is **write-only** (a password input,
cleared on submit) and the listing shows only the **redacted preview** (`…cdef`) + a
Delete action. The **MCP servers** form has a **Transport** selector (loopback HTTP
endpoint vs. managed stdio command); the stdio form takes a **command**, **args (one
argv element per line)**, an **Environment** field (one `ENV_VAR=secret_name` per line —
the right side is a **secret name**, a reference, never a value), and an optional
**Working directory** (inside the safe workspace root), all pre-checked with the same
fail-closed rules the kernel enforces (`apps/dashboard/src/plugins.ts`
`validateMcpRegisterDraft` / `mcpRegisterBody` / `mcpEnvFromText`).
The servers list shows each server's transport (`http` / `stdio`), its
`transport_display` (endpoint or `cmd args…`), the honest `configured`/`disabled` config
status, and **Discover** + **Remove** actions. For a managed-stdio server a **Process**
control row also shows the live process **status** (state badge with pid, start time,
tools-discovered count, redacted **last error** + **log tail**) and **Start** / **Stop**
/ **Restart** buttons (`ManagedStdioControls`). The **Resources** and **Prompts**
actions are shown for **both** transports (managed stdio reads resources + prompts
read-only too — see "Managed-stdio MCP resources (v1)" and "MCP prompts (v1)" above);
Prompts runs a live `prompts/list` and an inline `prompts/get` with a minimal arguments
form.
(`apps/dashboard/src/pages/Plugins.tsx`, `apps/dashboard/src/plugins.ts`,
`managedStdioStatusBadge`.)

### Remaining gaps (honest)

- **A fatal call tears the process down.** A `tools/call` / `tools/list` that **times
  out** or hits a transport error drops the managed process (the response stream may be
  desynced) and marks it `failed`; the operator (or the next fallback Discover/invoke)
  restarts it. This is deliberate — it avoids reusing a desynced pipe — but a single
  slow call can end a warm process.
- **Env + `cwd` are now supported (safely).** A managed-stdio server can reference
  stored secrets in its `env` and set a plugin-relative `cwd` — see "Local secrets &
  environment" below. The config still stores **no plaintext** (only secret
  *references* + a path), the resolved values are never serialized/logged, and a `cwd`
  must canonicalize INSIDE the configured safe workspace root.
- **Resources + prompts are bridged (read-only); sampling is now a gated subset.**
  `resources/list` / `resources/read` (resources v1) and `prompts/list` / `prompts/get`
  (prompts v1) work over **both** loopback HTTP and managed stdio — read-only
  context/templates only. MCP **sampling** (`sampling/createMessage`) is now implemented as
  a **gated, default-deny safe subset** on managed-stdio sessions (see "MCP sampling (v1)"
  above): off by default, capability-gated, provider-key-isolated, bounded, redacted, and
  audited, with a clean JSON-RPC refusal when disabled / no provider / over bounds (a
  server is never left hanging). Still **not** bridged: resource **subscriptions** (live
  change notifications), and a *per-request* interactive sampling approval (v1 is
  policy-based allow/deny — see the "Why per-request UI approval is future" note above).
- **Status polling, not push.** The dashboard reads status on demand (no live stream);
  a crash between reads surfaces on the next status read / call, not instantly.

## Importing a repository as a plugin (the "add a plugin" flow)

Adding a plugin is an explicit operator action on the **Plugins** page → **+ Install**.
Three sources, all governed, none of which **execute repository code at install time**:

- **GitHub URL** — `POST /v1/relux/plugins/install-github { url }`. Validated
  (`https://github.com/owner/repo[.git]`, no embedded credentials), then cloned with
  `git clone --depth 1` (argv-only, no shell) on the Relux host.
- **ZIP upload** — `POST /v1/relux/plugins/install-zip` (multipart). Extracted +
  validated on the host; path-traversal entries are refused.
- **Local folder** — `POST /v1/relux/plugins/install-dir { path }`. Read on the Relux
  process host (not the browser machine).

What install does with the source (`crate::plugin_install`):

- **No `relux-plugin.json` is required.** That file is **optional** — only first-class
  Relux plugins ship one, and almost no external repo will. This is the **common case**:
  with no manifest, Relux generates a safe **metadata-only wrapper** (no runnable tools,
  `TrustLevel::Unverified`, author sentinel `relux (generated manifest)`), scans it for
  read-only **hints** (MCP server, npm/python entrypoints, scripts, README), and lets the
  operator configure tools / register an MCP server afterward (see the next section).
  Arbitrary plugin code is never executed by install. The generated id is derived from the
  repo/folder/zip name, sanitized and collision-safe (`relux-plugin-<seed>`).
- If the source **does** happen to carry a `relux-plugin.json` manifest (a first-class
  Relux plugin), it is validated and installed directly with its declared tools.

The wrapper-vs-native distinction is surfaced honestly on the install result + plugin
row: the install API returns a `generated: bool` flag (`relux_kernel::is_generated_manifest`)
and `tool_count`, so the dashboard labels a generated import **"Imported as metadata-only —
no Relux manifest needed"** and never as a failure or a "manifest required" error.

**The install result card is itself actionable** (`apps/dashboard/src/pages/Plugins.tsx`
`InstallResultCard`). It does not just point at the row's Configure affordance — it offers
the LIVE next-action buttons inline, **reusing the same components a plugin row mounts** (no
duplicated authority): for a metadata-only wrapper it **auto-opens** the in-card
`ManifestPanel` (the read-only "Detected in source" hints, the **Register MCP server…**
proposal flow, and **Add a tool** definition) so the next step is immediate; a real ToolSet
with tools gets a **Runtime** button (the inline loopback `RuntimePanel`); an Adapter import
gets **Configure on Crew** (a link to `/crew`); and every result offers **Copy install path**
and **Install another / Done**. Nothing is auto-run and no new backend route is added — the
card is a faster on-ramp to the existing, gated configure surfaces.

The **GitHub URL** field is forgiving: it accepts the `owner/repo` shorthand as well as a
full `https://github.com/owner/repo` URL. The pure, conservative `normalizeGithubUrl`
(`apps/dashboard/src/plugins.ts`) expands **only** the exact `owner/repo[.git]` shape to the
canonical https URL and passes anything else through untouched — it never injects credentials
and never rewrites a scheme, so the kernel's authoritative `validate_github_url` stays the
real gate.

So "**Clone `nousresearch/hermes-agent` and import it as a plugin**" is a Plugins →
**+ Install** → **GitHub URL** action — paste the repo URL (or just `owner/repo`) and
install. It is **not**
a Prime *task*: Prime's local adapter is deterministic and cannot clone/import (below).

### Local Prime cannot clone/import — it fails closed with guidance

If a user instead phrases this as a Prime-chat *task* ("clone … and import it as a
plugin, and run it"), Prime creates the task on its **local** adapter, which is
deterministic and performs no external work. Rather than sit forever in `Running`
(the old "running but nothing happens" bug) or fake-echo it as done, the run now
**fails closed**: it reaches a terminal `Failed` (classified `adapter_missing`), the
task is parked **`Blocked`**, and the transcript + the Work page's recovery card carry
actionable guidance — **Open Plugins** (the import flow above) or **Reassign** to a
configured Claude/Codex adapter. See `docs/RELUX_MASTER_PLAN.md` §8.1 "Local Prime is
deterministic — it fails closed on real external work."

## MCP hint → review → register (one-click from imported plugin details)

An operator who imports an arbitrary repo with no `relux-plugin.json` gets a
metadata-only wrapper plus the read-only "Detected in source" hints
(`docs/mcp.md` is silent on these; see `crate::introspect::detect_hints`). When a hint
flags the source as a **likely MCP server** (`mcp-server` / `mcp-config`), the plugin
details now offer a **"Register MCP server…"** action that turns that detection into a
**pre-filled, reviewable** registration on the EXISTING loopback registry — never an
auto-action, and never running the source.

- **Proposal (read-only, fail-closed).** The same `/v1/relux/plugins/:id/hints`
  scan additionally builds a `relux_kernel::McpRegistrationProposal`
  (`crate::mcp_proposal::propose_mcp_registration`) **only when an MCP signal was
  detected**. It reads the same bounded metadata files the hint scan reads and
  **executes nothing**. It proposes a **sanitized, valid** server id
  (`relux_core::sanitize_mcp_server_id` — from the npm `package.json` `name`, else the
  plugin id, else `imported-mcp`; always passes `is_valid_mcp_id`), a description (from
  `package.json` `description`, else an honest default), and — **only when an MCP config
  file names a loopback `url` that passes `validate_loopback_url`** — a pre-filled
  endpoint. A non-loopback / `https` / missing `url` is **never** pre-filled.
- **stdio `{command, args}` pre-fills a reviewable managed-stdio draft.** Mirroring the
  Hermes MCP config shape (`reference/hermes-agent-main/hermes_cli/mcp_config.py` — a
  server is `{"url"}` HTTP or `{"command","args","env"}` stdio), a detected stdio command
  now **pre-fills a managed-stdio registration draft** (`suggested_transport =
  "managed_stdio"`, `detected_command`, `detected_args`) — advisory, requiring the
  operator's review/confirm (see "Managed stdio MCP servers" above). Relux still
  **executes nothing on import**, never uses the command as an HTTP endpoint, and only
  spawns it (argv-only, never a shell) after the operator registers it and clicks
  Discover/invoke. When no loopback endpoint can be safely inferred, the proposal sets
  `endpoint_required` so an *HTTP* registration would force manual entry — but the
  operator can instead register the pre-filled managed-stdio command.
- **The action opens a review form, not a runner.** The plugin details render a
  **"Register MCP server…"** button (`apps/dashboard/src/pages/Plugins.tsx`
  `DetectedHints` → the pre-filled `AddMcpServerForm`, seeded by the React-free
  `apps/dashboard/src/plugins.ts` `mcpDraftFromProposal`). The operator confirms/edits the
  id, transport, endpoint **or** command/args, and description; the detected command +
  honest notes show above the fields as advisory text. Submit is pre-checked with the
  SAME fail-closed rules the kernel enforces (`validateMcpRegisterDraft` mirrors
  `is_valid_mcp_id` + the required endpoint for HTTP, and the argv-only command/args
  rules for stdio), then POSTs the **existing** `POST /v1/relux/mcp/servers` route —
  **no new backend, no parallel registry, nothing auto-registered or auto-run**.
- **After registration: discover through the gate (unchanged).** On success the form
  points the operator to the **MCP servers** section to click **Discover**, which runs the
  existing live `tools/list` (`GET /v1/relux/mcp/servers/:id/tools`) and lists tools with
  their honest readiness. No tool is auto-enabled: a discovered tool stays the fail-closed
  Medium + Required (`needs_approval`) until the operator classifies it — exactly the
  existing behavior. The hint never becomes a runnable tool on its own.

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

### Single MCP tool invocation from Prime chat (same gates, no new path)

Beyond the operator-driven Plugins invoke and the task `tool_call` / `tool_plan` run
paths, an operator can ask **Prime in chat** to run ONE MCP tool by naming it
explicitly. This is a single tool invocation — **not** the (inert) multi-step plan
PROPOSAL below, and not a brain freely choosing tools.

- **Exact ref syntax.** `mcp:<server>/<tool>` — the stable synthetic `mcp:<server>`
  plugin id (mirroring openclaw's `mcp:<serverId>:<toolName>` ref in
  `src/tools/execution.ts`) plus the discovered tool name. Recognized phrasings:
  `use mcp:loopback/status.summary`, `call mcp:fs/search with {"q":"files"}` (inline
  JSON becomes the `arguments`), or a bare `mcp:fs/search`. Recognition reuses the SAME
  `crate::prime::parse_tool_request` resolver the plan path uses; `classify_intent`
  routes a single MCP ref to `PrimeIntent::ToolInvocation` (the multi-tool plan path
  already claims a message that names ≥ 2 tools behind a plan/sequence cue).
- **Normal chat never invokes.** A greeting, an insult/frustration, a vague idea, or a
  deliberative question about an MCP tool ("should I use mcp:fs/search?", "what does
  mcp:fs/search do?") resolves to NO tool — the bare-ref recognition is gated by
  `is_chat_guarded` (an explicit invoke verb forces it; otherwise the message must not be
  a guarded question/musing/venting). An MCP catalog merely being available never turns
  chat into an invocation.
- **Grounded against the live catalog, off-lock.** `prime_invoke_tool` resolves the ref
  against the SHARED `KernelState::live_tool_catalog` — installed plugin tools PLUS the
  live MCP-discovered tools the server pre-fetched OFF-LOCK for this turn
  (`discover_proposal_mcp_catalog`, the same `mcp:`-token-gated prefetch the plan proposal
  uses, injected via `set_proposal_mcp_catalog`). The kernel lock never spans the
  network read.
- **Same gates, same shaped result.** A resolved tool runs through the EXACT
  `invoke_tool` path above — permission (`tool:mcp-<server>:<verb>`), the
  risk/approval gate with the per-call / allow-always-grant bypass, audit — and returns
  the same **shaped** `{ "result": <text>, "structuredContent"?: … }` (the raw JSON-RPC
  envelope is never surfaced). The reply carries it on the turn's `invoked_tool`
  (`mcp:<server>/<tool>`, the source/tool label) + `tool_output` fields.
- **Gated → a staged approval card, never a dead-end refusal (never auto-allowed).** An
  unclassified / Medium+Required MCP tool is `needs_approval`, so Prime **does not run it
  directly**. Instead of dead-ending, the turn **stages a pending per-call approval** bound
  to the EXACT call and surfaces a compact approval card (see "Chat-staged approval"
  below). If a standing **allow-always grant** already authorizes this exact call, Prime
  runs it directly through the audited `invoke_tool` path instead (no second prompt — the
  operator already authorized it). Nothing is ever auto-approved from chat, and no
  bypass/dangerous flag is ever passed to an adapter.
- **Honest failures stay honest.** A tool the server does not advertise, an
  unreachable/disabled server, or an unregistered server each surface a clean, MCP-aware
  `tool_error` on the turn (no blank page, no raw JSON dump) — these never stage an
  approval (there is nothing real to approve).

### Chat-staged approval (gated tool → pending approval card → existing routes)

When an EXPLICIT chat tool invocation (a single `mcp:<server>/<tool>` ref, or an explicit
plugin-tool ref — see "Supported explicit tool-ref syntax" below) resolves to a gated
(`needs_approval`) tool and there is no standing allow-always grant, the chat path is now
**usable** rather than a dead end. It reuses the EXISTING per-call approval machinery end to
end — it invents no parallel security system (`docs/RELUX_MASTER_PLAN.md` §7.4 per-call
approval; openclaw `src/acp/permission-relay.ts` allow-once / allow-always / deny).

**Supported explicit tool-ref syntax (chat).** An operator names a tool to run by typing one
of these forms after an invoke verb (`use` / `run` / `call` / `invoke` / `execute` / `test`);
the ref is resolved fail-closed against the live catalog, so an id that is not installed/live
returns an honest "no such tool", never a raw dump (`crate::prime::parse_tool_request`):

- `mcp:<server>/<tool>` — a live MCP-discovered tool under the stable `mcp:<server>`
  synthetic plugin id (mirroring openclaw's `mcp:<serverId>:<toolName>` ref).
- `relux-tools-<plugin>/<tool>` — the bundled/installed `relux-tools-*` convention
  (e.g. `relux-tools-github/github.create_pr`).
- `<plugin-id>/<tool.name>` — ANY registered plugin whose id is a hyphenated kebab id and
  whose tool name is dotted (e.g. `acme-crm/crm.lookup`) — so a plugin whose id is **not**
  `relux-tools-*` can still be named in chat (mirroring openclaw's `plugin:<pluginId>:<toolName>`
  executor ref, `src/tools/execution.ts`). The hyphen + dotted-tool guard is deliberate: it
  keeps ordinary prose pairs ("and/or", "client-side/server-side", "tcp/ip") from being
  mistaken for a tool ref. A single-word plugin keyword (`use the github tool`) still resolves
  through the keyword path as before. If a ref is ambiguous or unresolved, Prime asks a
  clarifying question / says which tools are available rather than guessing (fail closed).

- **Staging (kernel, fail closed).** `prime_invoke_tool`'s `NeedsApproval` arm calls
  `KernelState::request_tool_invocation_approval` (the same call the Plugins
  `/v1/relux/tools/request-approval` route uses): it re-checks the agent holds the tool's
  permission, re-confirms the tool actually requires approval, bounds the args, and binds a
  one-shot `PendingToolInvocation` to the exact `(agent, plugin, tool, args snapshot +
  SHA-256)` alongside a `Pending` `Approval`. **Nothing runs.** The turn comes back
  `disposition = awaiting_approval` carrying `PrimeTurn.pending_tool_approval`
  (`relux_core::PrimeToolApprovalRequest`: the approval id, the `<plugin>/<tool>` label, the
  source `mcp`/`plugin` + server id, the lowercase risk, the human reason, a bounded
  **secret-redacted** args preview, the required permission, and `allow_always_supported`).
  The raw args are never put on the turn.
- **Standing-grant fast path (§7.4 preserved).** Before staging, the arm checks
  `matching_persistent_grant_id` for this exact `(agent, plugin, tool, permission, risk)`. A
  match means the operator already chose "allow always", so the call runs **directly**
  through `invoke_tool` (which records the grant use + audits) and the turn is
  `disposition = executed` with the shaped `tool_output` — no card, no second prompt.
- **The card drives the EXISTING routes only (UI, `apps/dashboard/src/pages/Prime.tsx`
  `ApprovalCard`).** The chat renders a compact B&W card — tool + source badge, risk,
  reason, the redacted args preview — with exactly the decisions the existing machinery
  supports:
  - **Approve & run** → `POST /v1/relux/approvals/:id/decide {decision:"approved"}` then
    `POST /v1/relux/approvals/:id/execute` (the consume-once bound invocation runs exactly
    once, shaped result shown inline).
  - **Allow always** → `POST /v1/relux/approvals/:id/allow-always` (approves AND persists a
    standing grant so future matching calls skip the prompt), then `…/execute` once. Offered
    only when `allow_always_supported` (always true for a per-call tool approval today).
  - **Deny** → `POST /v1/relux/approvals/:id/decide {decision:"rejected"}`, which **drops
    the bound invocation** outright — the gated call can never run without a fresh approval.
  Nothing is auto-approved by showing the card; every decision flows through the unchanged
  permission/approval/grant/audit gates.
- **Normal chat never stages an approval.** Staging happens ONLY on an already-classified
  `ToolInvocation` turn whose tool is gated. A greeting, an insult/frustration, a vague
  idea/brainstorm, or a deliberative question about a tool resolves to no tool (gated by
  `is_chat_guarded`) — so it never reaches the `NeedsApproval` arm and never stages an
  approval.

## Prime Agent Loop (bounded think → tool → observe → respond, in chat)

The single explicit invocation above runs ONE named tool and stops. The **Prime Agent Loop** turns
that into a real, configurable agentic loop for chat: on an explicit tool-request turn the configured
brain may **call an allowed tool, observe its real output, and continue** — chaining tool calls and
folding what it learned into a useful final answer — all behind the SAME fail-closed gates
(Hermes/Codex-style `run_conversation`, but local and operator-bounded). It invents no second
security model: every execution still flows through `prime_invoke_tool`.

> **Note on limits (supersedes the original "v1" fixed caps).** The first cut of this loop shipped
> with tiny hard-coded caps (3 tool calls / 3 brain rounds). Those made Prime feel like a toy. They
> are **replaced by a configurable autonomy policy** (`relux_core::PrimeAgentPolicy`): a practical
> *standard* profile (default **12** tool calls, **18** brain rounds, **180s**) and a higher
> *extended* profile (default **64** / **96** / **1800s**) the operator can tune, plus an explicit
> "keep working / extended mode" the user can invoke for long-running work. There is **no infinite
> loop** — see "Why bounded, not infinite" below.
>
> **Other toy caps were swept too.** The same "tiny hard constant where a serious product needs a
> bounded-but-practical limit" mistake was audited across the whole relux-\* layer and recorded in
> `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md`. The orchestration step cap (`6` → the named, shared
> `relux_core::MAX_ORCHESTRATION_STEPS = 16`) and Prime's **read-only** context loop bound
> (`MAX_TOOL_ROUNDS` `4` → `8`) were first raised to honest constants, then **folded into the same
> `PrimeAgentPolicy` autonomy dial** so an operator can tune them per deployment: `max_orchestration_steps`
> (standard **16**) / `extended_max_orchestration_steps` (**64**) and `max_context_rounds` (standard
> **8**, aligned with `MAX_TOOL_ROUNDS`) / `extended_max_context_rounds` (**32**), each clamped to a
> shared ceiling (**64**). The planner now takes the configured width as an argument
> (`plan_orchestration_with_limit`) so the deterministic create-path (`prime_orchestrate`) and the
> brain-proposal path (`reconcile_orchestration_slots`) read the SAME width and never drift; an
> over-width goal's overflow note **names the active limit and how to raise it**, never a silent
> drop. The read-only loop / up-front read executor take the resolved round budget
> (`context_rounds`), preserving the no-progress / repeat early-stop. Both stay finite safety rails.
> Real guardrails (the clamped autonomy ceilings, the loopback/size bounds, the echo fixture's
> demotion to dev/test-only) are deliberately kept — they are guardrails, not toy caps.

**When the loop engages (the safety wall).** Only when (a) a brain is configured (not Local) and
(b) the deterministic classifier returns `ToolInvocation` for the message (the user explicitly asked
to use / check / call a tool — the SAME gate the single invocation uses), and never on a
continuation. Normal chat, a greeting, frustration / profanity, a vague idea, a Q&A or a brainstorm
classifies as some other intent and **never enters the loop** — it stays conversational on the
unchanged reply path. When the brain (inside the loop) chooses not to call any tool, the loop
returns nothing and `run_prime` falls back to the normal turn path unchanged.

**The bounded loop (`crates/relux-kernel/src/prime_agent_loop.rs`, pure + unit-tested).**
- The brain is offered a **live, per-agent catalog** (`build_agent_catalog`) projected fail-closed
  from the shared live catalog (installed plugin tools + off-lock-discovered live MCP tools): ONLY a
  `Ready` (directly runnable) or `NeedsApproval` (runnable behind the approval/grant gate) tool the
  agent can actually run is offered. A tool the agent lacks permission for, or with no runtime, is
  omitted — the brain can never pick a tool that cannot run. This catalog is the loop's
  `valid_tool_names` (Hermes), and every pick is validated against it (`interpret_agent_reply`):
  an off-catalog / made-up name is fed back as a self-correction note, **never executed**.
- Each round the brain replies with strict JSON — either `{"tool":"<plugin/tool>","args":{…}}` to
  call one tool, or `{"answer":"…"}` to finish. A valid pick is executed through the gate
  (`KernelState::prime_agent_step` → `prime_invoke_tool`); the **real, redacted, bounded**
  observation is fed back, and the brain calls another tool or answers.
- **Configurable limits (Hermes `max_iterations`, but operator-set).** The per-turn ceilings come
  from the operator's `PrimeAgentPolicy`, resolved into `AgentLimits` (`max_tool_calls`,
  `max_brain_rounds`) for the active profile. The kernel also enforces an optional wall-clock
  deadline (`max_duration_secs`) via `AgentLoop::mark_deadline_exceeded` (the loop owns rounds/calls;
  the kernel owns the clock). The **standard** profile is used by default; the **extended** profile
  is selected when the user explicitly asks Prime to keep working (`prime_wants_extended_work` cue
  detection — a fallback keyword rail that only RAISES the ceiling for an already-`ToolInvocation`
  turn; it never creates a tool request). A repeated identical call (no progress) still stops the
  loop early; each observation is secret-redacted (`relux_core::redact_secrets`) and clamped
  (`MAX_OBS_CHARS`).
- **Limit reached → say so + offer to RESUME, never "done".** When a configured ceiling is hit
  with work still to do, the loop returns `AgentOutcome::LimitReached(LimitKind)` (tool-calls /
  reasoning-rounds / time). The turn's reply names EXACTLY which limit was reached, shows what was
  gathered so far, and the response carries a **resumable continuation handle** (`prime_continuation`
  — a stable `cont_NNNN` token + the pause reason + how many observations were gathered). The
  dashboard's one-click **"Keep working (extended)"** button POSTs that token to
  `/v1/relux/prime/agent/continue`, which RESUMES the same loop from the stored observations under a
  fresh budget — it does NOT re-send the original text. Prime never fabricates a finished answer it
  did not reach. See "Resumable continuation" below.
- **Gated tool → pause, not auto-run.** A `NeedsApproval` tool with no standing grant returns the
  EXISTING staged approval card (see "Chat-staged approval" above) and the loop **stops** — nothing
  ran. An allow-always grant turns that same tool into a direct run (the grant fast-path in
  `prime_invoke_tool`), so a granted tool participates in the loop like any low-risk one.
- **Fail closed on the result.** A missing / not-implemented / missing-permission / unknown tool is
  an honest refusal turn (no fabricated observation). A tool that ran but errored is recorded as an
  `ok:false` observation, never a fabricated success.

**Locking.** Every brain round runs OFF the kernel lock; every execution takes its own short lock
and is persisted (`drive_prime_agent_loop` in `server.rs`), so the kernel lock never spans a
network/process brain call. The loop is bounded, so the interleave is too.

**UX (no raw envelopes).** A multi-step turn surfaces a compact `tool_trace` (one chip per real
execution, `relux_core::PrimeToolTrace` → `apps/dashboard/src/pages/Prime.tsx` `ToolTrace`); a
single tool still shows its result via `ToolResult`; a paused turn shows the approval card. The
final reply is the brain's answer grounded in the observations (kept deterministic — `invoked_tool`
is set, so it is actionful and never re-narrated). No raw CLI JSON or transport envelope reaches the
user.

**Why bounded, not infinite (binding).** Even the extended profile has a finite ceiling, and the
operator cannot set "infinite" — every policy field is clamped (`PrimeAgentPolicy::clamped`: tool
calls ≤ 512, brain rounds ≤ 1024, duration ≤ 24h). A literal unbounded loop is unsafe: a misbehaving
brain or a runaway tool chain would spin forever, run up cost, and never yield control. The product
instead gives **operator-controlled high limits + an explicit continue**: when a ceiling is reached
the loop stops, reports it, and offers to continue — so "keep working" is a governed, auditable
continuation, not an ungoverned loop. Approvals still pause the loop and a high-risk tool never
auto-runs, regardless of the limits.

**Tool-plan step limit (same policy).** The same `PrimeAgentPolicy` also carries the configurable
**multi-tool-plan step limit** — `max_tool_plan_steps` (standard, default **16**, aligned with the
orchestration width) and `extended_max_tool_plan_steps` (default **64**), each clamped to the
absolute ceiling `MAX_TASK_TOOL_PLAN_STEPS_CEIL` (**64**). This replaces the retired hard-coded toy
`5`. It bounds an operator-authored / Prime-proposed [`TaskToolPlan`](#run-driven-multi-tool-plan):
the task-create route and the Prime tool-plan proposal both validate against the configured standard
limit via `TaskToolPlan::validate_with_limit`, and an over-limit plan is an honest `400` / blocking
issue that **names the limit and how to raise it** — never a silent truncation. The permissive read
path (`parse_task_tool_plan`, run execution) bounds only at the absolute ceiling, so a plan created
under a raised limit still reads back. (The static `MAX_TASK_TOOL_PLAN_STEPS` (16) is the
conservative default `validate()` uses in tests/CLI where no policy is threaded.)

**Orchestration width + context rounds (same policy).** The same `PrimeAgentPolicy` also carries the
configurable **orchestration fan-out width** — `max_orchestration_steps` (standard, default **16**) /
`extended_max_orchestration_steps` (**64**), clamped to the shared `MAX_ORCHESTRATION_STEPS_CEIL`
(**64**) — and the **read-only context-loop round budget** — `max_context_rounds` (standard, default
**8**, aligned with `MAX_TOOL_ROUNDS`) / `extended_max_context_rounds` (**32**), clamped to
`MAX_CONTEXT_ROUNDS_CEIL` (**64**). The deterministic `prime_orchestrate`, the brain
`reconcile_orchestration_slots`, and the preview route all pass the resolved
`orchestration_steps(false)` width into `plan_orchestration_with_limit`, so they never drift; the
read-only `ContextLoop` / `execute_requested_reads` take the resolved `context_rounds(false)` budget
(threaded from the server preview block into the observe-then-act `DecisionLoop` and the sidecar
loop). Both surface in the same places as the other policy fields.

**Background-job concurrency (resource guardrail, same policy).** The same `PrimeAgentPolicy` also
carries the configurable **concurrent background-job admission cap** for the async `run-async`
orchestration-job path — `max_active_jobs` (standard, default **4**) / `extended_max_active_jobs`
(**16**), clamped to the absolute `MAX_ACTIVE_JOBS_CEIL` (**64**). Unlike the per-turn dials above
this is a **fleet-wide RESOURCE guardrail**: each admitted job drives live adapter processes on its own
OS thread, so the cap bounds concurrent *load*, not how far one turn reasons — but it is the same kind
of named, raisable, clamped policy field (the Hermes precedent is the api-server's configurable
`max_concurrent` admission knob, not a hidden wall), replacing the retired hidden `MAX_ACTIVE_JOBS = 4`
constant. The `run-async` route reads the resolved `active_jobs(extended)` value and passes it into
`JobRegistry::start` (the registry no longer hard-codes a number); a request opts into the higher
profile with `{"extended": true}`. When the fleet is full the `429` **names the configured limit and
how to raise it** — the extended retry, the exact policy field, and the route — never a generic refusal.
Even "extended" is bounded, so a request burst can never spawn unbounded workers.

**Configuring + continuing.** The policy is served at `GET/PUT/PATCH /v1/relux/prime/agent-policy`
(response carries the resolved standard/extended limits, including `max_tool_plan_steps`,
`max_orchestration_steps`, `max_context_rounds`, and `max_active_jobs`), set in the dashboard's **Prime
Autonomy Limits** panel (Health → Prime Brain) — which now has **Tool plan**, **Orchestration**,
**Context loop**, and **Active jobs** rows for the standard/extended limits — or via `relux-kernel prime
agent-policy <status|configure>` (flags `--max-tool-plan-steps N` / `--ext-max-tool-plan-steps N`,
`--max-orchestration-steps N` / `--ext-max-orchestration-steps N`, `--max-context-rounds N` /
`--ext-max-context-rounds N`, `--max-active-jobs N` / `--ext-max-active-jobs N`). To run long
work: tell Prime to "keep working" / "use extended mode" (raises this turn to the extended profile),
or click the **Keep working (extended)** button Prime offers when a limit is hit (which resumes the
paused loop — see below).

### Resumable continuation (the real "keep working")

A paused agent-loop turn is RESUMABLE, not re-run. When the loop stops with work still to do — a
configured ceiling was reached, or a gated tool is waiting on approval — the kernel persists a small,
bounded **continuation** record for that conversation and stamps a `prime_continuation` handle on the
response.

- **What is persisted** (`relux_core::PrimeAgentContinuation`, in the kernel snapshot keyed by
  `namespace::actor`): a stable `cont_NNNN` **token**, the original request, the profile it ran under
  (standard / extended), the **already-gathered observations** (each bounded + secret-redacted, with
  its call signature), the pause reason, and — for an approval pause — the staged approval id. It is
  bounded: one record per conversation, a TTL (`PRIME_CONTINUATION_TTL_SECS`, 30 min), at most
  `MAX_PRIME_CONTINUATIONS` conversations, `MAX_CONTINUATION_STEPS` steps each.
- **Resuming** (`POST /v1/relux/prime/agent/continue` `{ continuation_id, extended? }`): the kernel
  validates the token (a stale / unknown / expired token **fails closed** with a clean reply — no
  resume), CONSUMES the record, and seeds `AgentLoop::resume` with the stored observations. The brain
  sees those results in its prompt and **does not re-run the already-completed calls** (each is
  skipped by call-signature, with a self-correction nudge to use the existing result or pick another
  tool). The resumed loop runs under a **fresh per-turn budget** (and the extended profile when
  asked), so it proceeds PAST where it stopped — behind the SAME gates as a fresh turn. If it pauses
  again it persists a fresh continuation. This is the real "pick up where it left off", not a blind
  re-run.
- **Approval resume (automatic).** When the loop pauses on a gated tool, the continuation records the
  staged approval. The operator approves + runs it through the **unchanged** approval routes
  (`/v1/relux/approvals/:id/{decide,execute,allow-always}` — nothing is auto-approved); on success
  `execute_approved_tool_invocation` **folds the real result into the waiting continuation** and
  clears the approval marker. The next "Keep working" then resumes with that result already in
  context. Denying the approval drops the continuation (it can never resume on a refused tool).

**Remaining v2 gaps (honest).** The continuation is per-conversation (a new paused loop replaces the
prior record for that conversation; older steps beyond `MAX_CONTINUATION_STEPS` fold into the reply
already shown rather than being re-fed). The resume does NOT yet: stream tool output live; branch /
run tools in parallel; carry the brain's intermediate *reasoning* tokens across the pause (only the
tool observations are carried — the brain re-reasons from them); or let the brain pick tools the user
did not explicitly request (the `ToolInvocation` gate is still the entry condition, and a continue
only resumes an EXISTING paused loop — normal chat never creates one). These stay out of scope to
keep the loop local, bounded, and reuse-only.

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
  event surfaces in the Work run detail's existing Transcript table. A later slice
  added a compact operator form for this (a single-step "Create a tool-run task" posts
  exactly this `tool_call`) — see "Run-driven multi-tool plan" → "Operating it" below.

**Scope (deliberately narrow).** This is a **deterministic, operator-named** run path:
one tool, fixed in the task input, gated end-to-end. It is NOT a brain freely choosing
arbitrary MCP tools mid-run — that broader autonomy stays out of scope until it routes
through an allowlisted/validated write tool and the same approval gates.

## Run-driven multi-tool plan (bounded, operator-authored sequence)

The single tool-call directive above drives exactly ONE tool. A task may now instead
carry a bounded, **operator-authored multi-tool plan** so a single run executes a small
fixed SEQUENCE of gated tool calls — the next step up in deeper multi-action loops,
still without a brain choosing the tools.

- **The plan.** A task's `input` may be the canonical shape
  `{ "tool_plan": [ { "plugin": "mcp:<server>"|"<plugin-id>", "tool": "<name>",
  "args": { … } }, … ] }` (`relux_core::TaskToolPlan` / `parse_task_tool_plan`). Each
  step is the same `{plugin, tool, args}` directive shape, where `plugin` is a synthetic
  `mcp:<server>` MCP server **or** a real installed plugin id. The plan is **fixed at
  task creation** — the brain never adds, removes, reorders, or chooses a step.
- **Bounds + strict create-time validation (`TaskToolPlan::validate_with_limit`, fail
  closed).** A plan must be **non-empty** and carry **at most the configured tool-plan
  step limit** steps. That limit is the operator's
  `PrimeAgentPolicy::max_tool_plan_steps` (standard, default **16** — aligned with the
  orchestration width, replacing the retired toy `5`), clamped to an absolute hard ceiling
  `MAX_TASK_TOOL_PLAN_STEPS_CEIL` (**64**, also the extended default). The static
  `MAX_TASK_TOOL_PLAN_STEPS` (16) is the conservative default `validate()` uses where no
  policy is threaded (tests/CLI). Every step must have a **non-empty plugin + tool**
  (trimmed); and every step's
  serialized `args` must be **≤ `MAX_TASK_TOOL_PLAN_ARGS_BYTES` (256 KiB)** — mirroring
  the loopback request cap so a step can never carry args `call_tool` would itself
  reject. An empty plan, an over-long plan (never silently truncated), an empty
  plugin/tool, or oversized args all **fail at create time** with an honest `400`
  (`POST /v1/relux/tasks`). `tool_plan` and the single `tool_call` are **mutually
  exclusive** — supplying both is a `400` (the run would take only one, so an unused
  intent must not be silently dropped).
- **Sequential execution, stop on first failure (`execute_local_run`).** When the
  deterministic local run sees a `tool_plan`, it executes each step **in order** through
  the SAME gated `call_tool` chokepoint (the same permission + risk/approval +
  per-call-approval / allow-always-grant + audit + `mcp_tool_call*` / `tool_call*`
  transcript path as a single directive). **Every step is gated independently** — a
  missing permission or a requires-approval step blocks the run at that step. The run
  **stops on the first step's failure or denial**, marking the run + task `Failed`
  (never a partial-success lie); on full success the run + task complete and the run
  summary is a **compact** `ran N tool step(s): <i/N: tool via plugin ok; …>` — the step
  count + per-step ok markers only, never the step results (which would risk a leak; the
  per-step result already lives on the transcript, redacted, via `call_tool`).
- **Transcript.** Each step appends its own existing per-tool event through `call_tool`
  — an `mcp_tool_call` (success, bounded redacted `result_summary`) /
  `mcp_tool_call_denied` / `mcp_tool_call_failed` for an MCP step, or the generic
  `tool_call*` for a plugin step. A two-step plan therefore shows two tool-call events,
  in order. No new run-event kind and no new UI surface is added in this slice — the
  plan reuses the existing transcript path end to end.
- **Operating it (compact UI on the Plugins → Tools section).** The Plugins page's
  **Tools** section now carries a small **"Create a tool-run task"** form
  (`apps/dashboard/src/pages/Plugins.tsx` `CreateToolRunTask`, payload built by the
  React-free `apps/dashboard/src/toolruntask.ts` `buildToolRunTaskPayload`). An
  operator gives the task a title and adds **1–5 steps**, each picking a tool from the
  picker and supplying optional **JSON args** (blank = `{}`). One step
  posts a `tool_call`; two-or-more post a `tool_plan` — the SAME existing
  `POST /v1/relux/tasks` body (no new backend). The builder fails closed the way the
  kernel does (title required, ≤5 steps never silently truncated, a tool must be
  chosen per step, args must be valid JSON) so the form never sends a request the
  kernel would `400`. It is **honest about approval**: a chosen gated
  (`needs_approval`) tool can be put in a plan, but the form shows a banner that the
  run will **block/fail** on that step unless a standing allow-always grant exists —
  it never implies auto-approval.
- **The picker includes MCP-discovered tools (not just installed plugin tools).** When
  the form is opened it lists the registered MCP servers (`reluxMcp.list`) and runs a
  live `tools/list` (`reluxMcp.tools`) against each **enabled** one, then merges those
  tools into the picker beside the installed plugin tools. The merge + honest notes are
  produced by the React-free `apps/dashboard/src/toolruntask.ts` `buildToolPickerOptions`
  (gating reuses the same `toolReadiness` rule the Tools list uses). An MCP tool is
  offered under the **stable plugin id `mcp:<server>`** with the discovered tool name,
  labelled `mcp:<server>/<tool>`, so picking it builds a directive the kernel routes as
  an MCP call (`plugin_id = "mcp:<server>"`, the SAME id the "Run-driven MCP tool call"
  path uses). Readiness is honest: an unclassified / `medium`+`required` MCP tool reads
  as **"needs approval"** (it can still be planned, but the run gates on it), exactly
  like a gated plugin tool. Discovery never hides a server silently — an **enabled
  server whose `tools/list` failed** shows a **warning** banner naming it (down /
  stopped / not speaking MCP — never a faked empty list), a **disabled** server shows
  an **info** note (enable it to discover), and a failure to even list the servers shows
  a warning that only installed plugin tools are available. Discovery is gated on the
  form being open, so merely loading the Plugins page never dials the operator's
  loopback servers; each open re-discovers (fresh truth, never cached).
- On success the form shows the new task id and a link to
  **Work**, where the operator runs it with "Run (Assigned)" /
  `POST /v1/relux/tasks/:id/execute-assigned`. The raw `POST /v1/relux/tasks` route
  (with the optional `tool_plan` / `tool_call` body) remains available for scripted
  use.

**Scope (still narrow).** A `tool_plan` is a **fixed, operator-authored** sequence,
validated and capped, gated per step. It is NOT a brain choosing tools mid-run, NOT
conditional/branching execution, NOT data-flow between steps (each step's `args` are
fixed at creation; a step cannot consume a prior step's output), and NOT cross-adapter
(it runs only on the deterministic local-prime adapter, like the single directive).
Those remain out of scope until they route through allowlisted/validated write tools and
the same approval gates.

### Brain-assisted tool-plan PROPOSAL (Prime preview, inert)

The operator can also reach the same bounded `tool_plan` task through **Prime chat**, as
a **reviewable proposal** — never an auto-action. **Prime is a Hermes-like general agent
first** (normal chat, Q&A, brainstorming, emotional support); the company/crew/tool
powers are optional abilities, not the default personality. So a greeting, an insult,
frustration, a vague idea, a question, or any casual turn **answers naturally and carries
no tool plan**. Only an **explicit ordered multi-tool command** ("run these tools in
order: …", "use the status tool then the echo tool", "chain these tools") produces a
preview.

- **Classification (safe classifier + validated LLM, fail closed).** The deterministic
  `classify_intent` recognizes `PrimeIntent::ToolPlanRequest` ONLY when an explicit
  plan/sequence cue is present AND **≥ 2 segments resolve to a real tool reference**
  (`crates/relux-kernel/src/prime.rs` `split_tool_plan_segments` + `parse_tool_request`).
  A single tool stays `ToolInvocation`; a "then" in ordinary chat resolves to no tools and
  never reaches here. The optional LLM brain may also propose the intent, but it is
  **sensitive** in the fail-closed reconcile gate (`prime_intent.rs`
  `is_sensitive_intent`), so the brain can never promote guarded chat (a greeting, an
  insult, a vague musing/question) into a tool plan — only an explicit command, which the
  deterministic classifier already catches, gets there.
- **Inert, grounded preview (`KernelState::build_tool_plan_proposal`).** The
  `ProposeToolPlan` action is READ-ONLY: the kernel splits the request into ordered
  steps and resolves each against the **shared live tool catalog**
  (`KernelState::live_tool_catalog`) — **installed plugin tools (`discover_tools`)
  PLUS the live MCP-discovered tools** from every enabled MCP server. It surfaces each
  step's honest `readiness` (`ready` / `needs_approval` / `missing_permission` /
  `not_runnable` / `unknown` / `unavailable`) and declared `risk`, and validates the
  whole bounded plan with the **same `TaskToolPlan::validate`** the create route enforces
  — **creating no task, running no tool, and mutating nothing**. An **unresolved step is
  never silently accepted**: an installed tool that does not exist, or an MCP tool the
  server does not advertise, is flagged `unknown`; a referenced MCP server that is
  **unreachable / disabled / unregistered** is flagged `unavailable` (fail-closed, with
  the reason). In every such case `ready_to_create` is `false` and the reply becomes a
  clarifying question; an **over-cap** plan (> 5 steps) is reported as too-long, never
  truncated silently. The preview ships as `PrimeTurn.tool_plan_proposal`
  (`relux_core::PrimeToolPlanProposal`: a human summary, the ordered steps, `ready_to_create`,
  and honest `issues`); it carries **no `PrimeAction`**.
- **Live MCP tools, off-lock + fail-closed.** A step is referenced as
  `mcp:<server>/<tool>` (the stable `mcp:<server>` synthetic plugin id, mirroring
  openclaw's `mcp:<serverId>:<toolName>` ref in `src/tools/execution.ts`). The HTTP
  server runs the bounded live `tools/list` **outside the kernel lock**
  (`discover_proposal_mcp_catalog`, gated on the message carrying an `mcp:` reference and
  ≥ 1 enabled server) and injects the result via `set_proposal_mcp_catalog` just before
  the locked turn — so grounding never holds the kernel lock across a network read (the
  same off-lock discipline as `context_snapshot`). A `tools/list` failure does **not**
  fake the tool: the server is recorded unavailable and the step grounds `unavailable`.
  An unclassified MCP tool reads `needs_approval` (fail-closed Medium + Required), exactly
  as in the unified Tools picker.
- **Explicit one-click commit, existing path + gates (UI).** The Prime chat renders the
  preview as a compact card (`apps/dashboard/src/pages/Prime.tsx` `ToolPlanCard`) under
  the assistant reply: ordered steps, tool names, readiness/risk badges, and a compact
  args preview. A **"Create tool-run task"** button (enabled ONLY when `ready_to_create`)
  POSTs the validated steps straight to the **existing** `POST /v1/relux/tasks` `tool_plan`
  route (`reluxWork.createTask`) — **no new backend, no magic phrase**. Execution still
  flows only through the existing tool-run task path and its **unchanged
  permission/approval/grant/audit gates** at run time; nothing runs until the operator
  starts the task in Work. The card is honest: nothing is created or run by showing it.

**Scope (proposal layer).** The proposal is grounded against the **shared catalog of
installed plugin tools + live MCP-discovered tools** (`KernelState::live_tool_catalog`),
so a `mcp:<server>/<tool>` step grounds against a real enabled MCP server exactly like an
installed plugin tool and lands in the SAME `mcp:<server>` task `tool_plan` execution path
(the operator's Plugins → Tools "Create a tool-run task" form uses the same merge). The
proposal chooses no tools on its own, runs nothing, mutates nothing, and adds **no new
execution path** — execution stays the single existing gated `tool_plan` task path. A
referenced MCP server/tool that is not live is reported `unavailable` / `unknown`, never
faked. Normal chat, brainstorming, and frustration still resolve to no tools and never
produce a plan.

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

GET    /v1/relux/secrets                                           list stored secrets (redacted status only — never a value)
PUT    /v1/relux/secrets/:name                                     { value } — set a secret (write-only; returns redacted status)
DELETE /v1/relux/secrets/:name                                     delete a secret ({ removed })
```

A managed-stdio registration may carry `env` (secret references) + `cwd`:
`POST /v1/relux/mcp/servers { "id", "transport":"managed_stdio", "command", "args"?,
"env": { "<ENV_VAR>": { "secret": "<name>" } }, "cwd"?, … }`. The `env` map stores only
secret **names**, never values; the value is supplied separately via the secrets routes
above and resolved into the child env at spawn (`docs/mcp.md` "Local secrets &
environment").

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
`parse_task_tool_call` — the operator-named single directive type/parser; **plus
`TaskToolPlan` + `TaskToolPlanError` + `parse_task_tool_plan` +
`TaskToolPlan::validate_with_limit` + the configurable
`PrimeAgentPolicy::max_tool_plan_steps` limit / `MAX_TASK_TOOL_PLAN_STEPS_CEIL` /
`MAX_TASK_TOOL_PLAN_ARGS_BYTES` bounds — the bounded multi-tool plan
type/parser/validator**), `crates/relux-kernel/src/state.rs`
(`execute_local_run` routes a single directive — **or, before it, each step of a
`tool_plan` in order, stopping on the first failure/denial** — through `call_tool`
instead of echo, failing the run/task honestly on a gate refusal),
`crates/relux-kernel/src/server.rs` (`create_task` / `CreateTaskReq` accept the optional
`tool_call` directive **or the optional `tool_plan` (validated strictly; mutually
exclusive with `tool_call`)** and serialize it into the canonical input).

**Multi-tool plan reference mapping (`docs/reference-driven-development.md`, BINDING).**
Files read before implementing the plan:
- **Hermes** `reference/hermes-agent-main/agent/agent_runtime_helpers.py` — the
  conversation tool loop iterates an assistant turn's `msg["tool_calls"]` list
  (`for tool_call in tool_calls`, L266-272), executing each tool call in sequence. We
  mirror the **sequential per-step execution** but deliberately **diverge on authorship**:
  Hermes' list is MODEL-chosen each turn; a Relux `tool_plan` is OPERATOR-authored and
  fixed at task creation, so the brain never chooses a step (the keyword/brain-free rail
  the design docs require here).
- **openclaw** `reference/openclaw-main/src/tools/planner.ts` (`buildToolPlan`) —
  validates an ENTIRE tool plan up front (unique names, availability diagnostics,
  executor presence) and partitions valid/invalid BEFORE any execution rather than
  discovering invalidity mid-run. We adopt the same **validate-the-whole-plan-up-front,
  fail-closed** posture in `TaskToolPlan::validate` (non-empty, step-count cap, per-step
  plugin/tool + args bounds checked at create time), so an invalid plan is a `400` and
  never a partially-run task.

**Prime Agent Loop reference mapping (`docs/reference-driven-development.md`, BINDING).**
Files read before implementing the loop and the configurable autonomy policy:
- **Hermes** `reference/hermes-agent-main/agent/conversation_loop.py` `run_conversation(...)` —
  the per-turn agentic loop: `while (api_call_count < agent.max_iterations and
  agent.iteration_budget.remaining > 0)` (L598) bounds the rounds; the assistant reply is inspected
  for tool calls, each requested tool is executed and its result fed back as a `role:"tool"` message
  (L630-676), and the loop ends when the model stops requesting tools and answers. The chosen tool
  is validated against `agent.valid_tool_names` BEFORE execution (L389, L656) — an off-list name is
  fed back as a self-correction message, never executed.
- **Hermes** `reference/hermes-agent-main/agent/iteration_budget.py` `IterationBudget(max_total)` +
  `cli-config.yaml.example` — the loop bound is a **configurable** per-agent budget (`max_iterations`
  default **90** for the parent, `delegation.max_iterations` default **50** per subagent), a
  thread-safe consume counter, NOT a tiny hard constant. This is the direct precedent for replacing
  Relux's toy 3/3 caps with a configurable policy: a real agent's per-turn ceiling is set high and
  operator-tunable. We mirror the *configurable-ceiling* shape (our `PrimeAgentPolicy` →
  `AgentLimits`, standard default 12/18 and extended 64/96) while keeping our explicit-`ToolInvocation`
  entry wall.
- We mirror the loop shape exactly in `crates/relux-kernel/src/prime_agent_loop.rs` — `AgentLoop`
  (the bounded driver; the per-turn ceiling now lives in `AgentLimits`, resolved from the operator's
  `relux_core::PrimeAgentPolicy`, NOT a fixed constant), the live `AgentTool` catalog
  (`valid_tool_names`), `interpret_agent_reply` (the pick interpreter + off-catalog `UnknownTool`
  self-correction), and a reply with no tool/`{"answer":…}` ends the loop. We **diverge** by gating
  loop *entry* on an explicit `ToolInvocation` turn (the safety wall) and by reporting a hit ceiling
  as `AgentOutcome::LimitReached` so the turn offers an auditable continue rather than pretending it
  finished.
- **openclaw** `reference/openclaw-main/src/agents/tool-mutation.ts` `isMutatingToolCall` (the
  fail-closed default — an unknown action is mutating) and `src/acp/approval-classifier.ts` (an
  unknown tool never auto-approves). We invert the polarity for the same safety: `build_agent_catalog`
  admits ONLY a `Ready`/`NeedsApproval` tool the agent can run (everything else omitted), a gated
  tool is never auto-run (the loop pauses with the existing approval card), and a stale/off-catalog
  pick is refused.
- **openclaw** `reference/openclaw-main/src/acp/permission-relay.ts` — allow-once / allow-always /
  deny. The pause reuses the EXISTING `PrimeToolApprovalRequest` card + routes unchanged; the loop
  only signals WHEN to pause.

Prime Agent Loop files: `crates/relux-kernel/src/prime_agent_loop.rs` (the pure driver + types +
unit tests), `crates/relux-kernel/src/state.rs` (`prime_agent_catalog` + `prime_agent_step` — the
loop's catalog + its ONLY execution path, reusing `prime_invoke_tool`),
`crates/relux-kernel/src/server.rs` (`drive_prime_agent_loop` + `agent_brain_round` +
`build_agent_loop_turn` — the off-lock-brain / short-locked-exec orchestration in `run_prime`),
`crates/relux-core/src/prime.rs` (`PrimeToolTrace` + `PrimeTurn.tool_trace`),
`apps/dashboard/src/pages/Prime.tsx` (`ToolTrace` chips).

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

**MCP hint → register files** (`docs/mcp.md` "MCP hint → review → register"):
`crates/relux-core/src/mcp.rs` (`sanitize_mcp_server_id` — the valid-id reducer),
`crates/relux-kernel/src/mcp_proposal.rs` (`McpRegistrationProposal` +
`propose_mcp_registration` — the read-only, executes-nothing draft builder + tests),
`crates/relux-kernel/src/server.rs` (`PluginHintsResponse.mcp_proposal` — folded into the
existing `/v1/relux/plugins/:id/hints` scan; registration reuses the unchanged
`POST /v1/relux/mcp/servers`), `apps/dashboard/src/plugins.ts` (`mcpDraftFromProposal` +
`validateMcpRegisterDraft` — React-free seed + fail-closed pre-check, unit-tested),
`apps/dashboard/src/pages/Plugins.tsx` (`DetectedHints` "Register MCP server…" → the
pre-filled `AddMcpServerForm` + `McpProposalAdvisory`).

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
server-push channel exists; (5) a **bounded multi-tool run plan** — DONE: an
operator-authored, create-time-validated `tool_plan` of ≤ 5 gated steps now runs
sequentially in one local-prime run, stopping on the first failure (see "Run-driven
multi-tool plan" above). What remains out of scope here: a **CLI-adapter** multi-tool
path (the plan runs only on the deterministic local-prime adapter), **data-flow /
conditional** steps (a step consuming a prior step's output, or branching), and a brain
freely choosing arbitrary MCP tools mid-run (still gated behind allowlisted/validated
write tools + the same approval gates).
