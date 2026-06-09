<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/relix-logo-dark.svg">
    <img src="docs/assets/relix-logo-light.svg" alt="Relix — Relay Intelligence Exchange" width="500">
  </picture>
</p>

<p align="center">
  <strong>The OS for AI Agents</strong>
</p>

<p align="center">
  <a href="https://github.com/itsramananshul/Relix/releases"><img src="https://img.shields.io/github/v/release/itsramananshul/Relix?include_prereleases&style=for-the-badge" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg?style=for-the-badge" alt="MIT OR Apache-2.0"></a>
</p>

---

## Relux (preview): one-command local control plane

Relux is the current product direction: a Prime-centered, plugin-first control
plane for agentic work that runs locally and serves its own dashboard. Boot it
with one command - no web bridge, no login:

```bash
cargo run -p relux-kernel -- serve
```

Then open the dashboard it prints:

```text
Relux dashboard: http://127.0.0.1:19891/dashboard
Relux API:       http://127.0.0.1:19891/v1/relux/state

Also available:
  GET /v1/relux/tasks/:id
  POST /v1/relux/tasks/:id/execute-assigned
  GET /v1/relux/runs/:id
  GET /v1/relux/runs/:id/events
  GET /v1/relux/audit?limit=N
  GET /v1/relux/tools
  POST /v1/relux/tools/invoke
```

### Tools (capability discovery + invocation)

Installed ToolSet plugins are surfaced as callable capabilities through the
kernel, CLI, API, and dashboard. The first version is deliberately safe and
honest: **only built-in deterministic tool handlers execute.** An
installed-but-unimplemented tool is listed as `not_implemented` rather than
faked, and arbitrary downloaded plugin code is never run.

Two built-in tools ship today:

- `relux-tools-echo` / `echo.say` - returns its input unchanged.
- `relux-tools-status` / `status.summary` - returns a deterministic summary of
  control-plane counts (read-only; no network or filesystem access).

Every invocation routes through the kernel permission check and the audit log.

```powershell
# List installed tools and whether the kernel can actually run each one.
relux-kernel tools

# Invoke a built-in tool as Prime (JSON input is optional, defaults to {}).
relux-kernel tool invoke relux-tools-echo echo.say '{"message":"hi"}'
relux-kernel tool invoke relux-tools-status status.summary
```

API:

```text
GET  /v1/relux/tools            # installed tools + executable status (?agent=<id> to scope)
POST /v1/relux/tools/invoke     # { "plugin_id", "tool_name", "input"?, "agent_id"? }
```

A permission denial returns HTTP 403; an installed tool with no kernel runtime
returns HTTP 501 (`ToolRuntimeUnavailable`) with a clear message and never
fabricates output. The dashboard Plugins page lists tools with their status and
provides a small invoke panel (JSON input + output/error) for ready tools.

#### Prime can use tools from chat

You no longer need to leave Prime for the Tools panel for simple, safe tool use.
Prime chat is tool-aware and runs the built-in tools through the **same**
permission/audit path as `/v1/relux/tools/invoke`:

- "what tools can you use?" - lists the installed tools with their honest
  executable status (grounded discovery; it never invents a tool).
- "give me a status summary" / "what is going on?" - consults
  `relux-tools-status/status.summary` and answers from the real output.
- "echo hello" / "use echo.say with {\"n\":1}" - runs `relux-tools-echo/echo.say`
  with the parsed input and shows the returned JSON.

Prime stays honest: a plain "hey" never becomes a tool call, and a request to use
an installed-but-unimplemented tool (e.g. a GitHub ToolSet) is reported as
"installed and discoverable, but this local runtime cannot execute it yet" with
no fabricated output. A missing permission is surfaced, never bypassed.
**Arbitrary downloaded plugin runtime execution remains intentionally not
implemented.** The CLI prints the invoked tool and its output too:

```powershell
relux-kernel prime "what tools can you use?"
relux-kernel prime "echo hello"
relux-kernel prime "give me a status summary"
```

The dashboard Prime page renders the invoked tool and its JSON output (or the
honest "tool not run" reason) compactly in the chat transcript.

### Plugin Runtime v1 (HTTP loopback ToolSet runtime)

By default only the built-in `echo`/`status` tools execute, and an installed
ToolSet plugin's tools are listed as `runtime_not_configured`. **Relux never
auto-runs downloaded plugin code** - it does not shell out to plugin commands,
does not run code from GitHub/zip/folder installs, and never calls a remote host.

A ToolSet plugin becomes executable only when an operator **explicitly points it
at a loopback HTTP server they run themselves**. The plugin author/operator runs
their own local server; Relux calls it through a narrow, permission-checked,
audited protocol.

The protocol is one stable endpoint:

```text
POST <base_url>/invoke
Content-Type: application/json
{ "plugin_id": "relux-tools-demo", "tool_name": "demo.ping", "input": <json> }

200 OK  { "output": <json> }     -> success
200 OK  { "error": "..." }       -> the tool refused/failed (surfaced honestly)
```

Safety limits (all enforced by the kernel):

- **Loopback only.** Only `http://127.0.0.1:<port>`, `http://localhost:<port>`, or
  `http://[::1]:<port>` are accepted - with an explicit port. `https`, remote
  hosts, embedded credentials, query/fragment, and `..` paths are rejected.
- **Bounded.** A per-call timeout (default 5000 ms, clamped 100-60000), a request
  body cap, and a response body cap. JSON in, JSON out. No TLS, no redirects.
- **Permission-checked + audited.** Every invocation passes the same kernel
  permission check and audit-log path as the built-in tools. A connection
  failure, timeout, non-200, oversized body, invalid JSON, or `{ "error": ... }`
  becomes a clear error - never a fabricated success.
- **No secrets stored.** The per-plugin config holds only the loopback base URL,
  the enabled flag, and the timeout.

Bundled plugins (`echo`/`status`) cannot be given a loopback runtime - they
already run as built-in deterministic tools.

CLI:

```powershell
# Show a plugin's runtime config/status.
relux-kernel plugin runtime relux-tools-demo

# Configure + enable an HTTP loopback runtime (optional --timeout-ms).
relux-kernel plugin runtime set relux-tools-demo http://127.0.0.1:19999 --timeout-ms 5000

# Disable the runtime (keeps the URL so it can be re-enabled).
relux-kernel plugin runtime disable relux-tools-demo
```

API:

```text
GET    /v1/relux/plugins/:id/runtime   # runtime status/config (no secrets)
PUT    /v1/relux/plugins/:id/runtime   # { "base_url", "enabled"?, "timeout_ms"? }
PATCH  /v1/relux/plugins/:id/runtime   # partial update (toggle enabled / timeout)
DELETE /v1/relux/plugins/:id/runtime   # clear the runtime config
```

`/v1/relux/tools` reflects the runtime status (`ready` once an enabled loopback
runtime is configured, otherwise `runtime_not_configured` / `runtime_disabled`),
and `/v1/relux/tools/invoke` routes configured loopback tools through the runtime.
A disabled runtime returns HTTP 409; a loopback failure returns HTTP 502; a
non-loopback URL is rejected with HTTP 400.

Dashboard: each non-bundled plugin on the Plugins page has a **Runtime** panel to
set the loopback URL + timeout, disable, or clear it. Configured-and-enabled
tools then show as `ready` in the Tools section and can be invoked from the
existing invoke panel; unconfigured tools show `runtime not configured` with a
configure affordance.

### Adapter Runtime v1 (local coding-agent CLIs)

An **Adapter** plugin decides how an assigned task runs. The bundled
`relux-adapter-local-prime` runs the deterministic echo path. Adapter Runtime v1
adds bundled adapters that drive a **local coding-agent CLI** you already have
installed:

- `relux-adapter-claude-cli` &rarr; runs `claude -p --permission-mode default`
- `relux-adapter-codex-cli` &rarr; runs `codex exec`
- any other Adapter plugin can be driven as a **generic command** by configuring
  an explicit binary.

Safe by construction:

- **Disabled by default.** Relux never spawns a paid/interactive CLI unless you
  explicitly enable that adapter's runtime (via CLI, API, or the dashboard).
- **No bypass.** Relux uses the Claude CLI's safe `--permission-mode default` and
  **never** passes `--dangerously-skip-permissions` or any danger/bypass flag.
- **argv only, prompt on stdin.** Commands are built as an argv array (no shell);
  the composed task prompt is fed on the child's stdin, so there is no arg-escaping
  surface and it works the same for native binaries and Windows `.cmd` shims.
- **Bounded + redacted.** Each run has a wall-clock timeout (the child is killed on
  expiry) and a stdout/stderr byte cap; captured output is scrubbed of obvious
  secrets before it is stored on the run transcript.
- **Honest failures.** If the adapter is disabled, unconfigured, the binary is not
  on PATH, it times out, or it exits non-zero, the run and task are marked
  **failed** with the reason on the transcript &mdash; never a fabricated success.
- **No secrets stored.** The per-adapter config holds only the kind/command,
  enabled flag, timeout, output cap, and an optional working dir.

The composed prompt includes the agent's name + persona and the task title/JSON
input, and asks the CLI to do the work and report concisely. The local Prime
adapter is not configurable here (it has no external binary). Prime autonomy keeps
running only the deterministic local path &mdash; it never spawns a CLI.

CLI:

```powershell
# List installed adapters + their runtime status (on-PATH, enabled, ...).
relux-kernel adapters

# Show one adapter's runtime status.
relux-kernel adapter runtime relux-adapter-claude-cli

# Enable a CLI adapter (optional overrides). Disabled by default until you do this.
relux-kernel adapter runtime enable relux-adapter-claude-cli --timeout-seconds 120 --max-output-bytes 1000000
relux-kernel adapter runtime enable relux-adapter-codex-cli

# Disable it again (keeps the config so it can be re-enabled).
relux-kernel adapter runtime disable relux-adapter-claude-cli
```

API:

```text
GET    /v1/relux/adapters                 # all adapters + runtime status (no secrets)
GET    /v1/relux/adapters/:id/runtime     # one adapter's status
PUT    /v1/relux/adapters/:id/runtime     # { "enabled":true, "command"?, "timeout_seconds"?, "max_output_bytes"?, "working_dir"? }
PATCH  /v1/relux/adapters/:id/runtime     # partial update
DELETE /v1/relux/adapters/:id/runtime     # clear the runtime config
```

A non-Adapter plugin or the local-prime adapter is refused (HTTP 400/404); a
disabled adapter on execute returns HTTP 409; a missing binary returns HTTP 422; a
failed/timed-out process returns HTTP 502.

Dashboard: the **Crew** page has an **Adapters** section showing each adapter's
status (local / disabled / enabled-ready / enabled-but-binary-missing) with an
Enable/Disable control and the clear note that *Relux will run this local CLI when
an assigned task starts*. On the **Work** page, the "Run (Assigned)" action now
dispatches through the assigned agent's adapter, and the run detail shows the
adapter's (redacted) output or the honest failure reason.

### Bundled plugins refresh idempotently (no reset needed)

The shipped bundled plugins and adapters (`relux-tools-echo`, `relux-tools-status`,
`relux-adapter-local-prime`, `relux-adapter-claude-cli`, `relux-adapter-codex-cli`)
are reconciled into your local store on **every** startup/load path - `doctor`,
`serve`, `plugins`, `adapters`, Prime/chat, and task execution all run the same
idempotent refresh. An existing local DB therefore picks up newly shipped
capabilities automatically, with **no `reset-local` required**:

- A missing bundled plugin is added (protected, non-removable).
- An existing bundled plugin whose shipped manifest changed is updated in place -
  no duplicate records, and your `enabled` choice and per-plugin runtime config are
  preserved.
- An up-to-date store is a no-op (no audit noise).
- A plugin you installed yourself is never overwritten, even if it shares an id
  with a bundled one.

`relux-kernel doctor` and `relux-kernel plugins` will refresh-and-save an older
store on the spot, so the new bundled plugins show up the next time you list them.

### First Local Release

Use the local release check before cutting or sharing a Windows bundle:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-first-release-check.ps1
```

Create a portable local bundle:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1
```

The package script writes `dist\relux-local-<version>-windows-x64\` and a zip
next to it. The bundle includes `relux-kernel.exe`, the built dashboard,
bundled example plugins, docs, and `Start-Relux.ps1`.

### Prime Autonomy

Prime can now keep safe local work moving while `relux-kernel serve` is running.
Autonomy is disabled by default. Enable it only when you want Prime to poll for
ready assigned tasks and execute them through the same governed assigned-run path
used by the Work page:

```powershell
relux-kernel prime autonomy status
relux-kernel prime autonomy configure --interval 60 --max-tasks 1 --auto-assign false
relux-kernel prime autonomy enable
relux-kernel prime autonomy tick
relux-kernel prime autonomy disable
```

The dashboard Prime page includes the same controls: enabled/disabled,
interval, max tasks per tick, optional auto-assignment, and a manual one-tick
button. Prime autonomy never installs plugins, grants permissions, deletes data,
or bypasses approvals.

### Optional LLM-backed Prime

By default, Prime is deterministic and rule-based. You can enable a natural,
LLM-backed chat path by configuring an OpenRouter API key. In this mode,
conversational replies (greetings, status, explanations) are shaped by the model
while actions (task creation, starting runs) stay grounded and deterministic in the
kernel.

1. Set `RELUX_OPENROUTER_API_KEY` to your OpenRouter key.
2. (Optional) Set `RELUX_OPENROUTER_MODEL` (default: `openai/gpt-4o-mini`).
3. (Optional) Set `RELUX_LLM_DISABLED=1` to force deterministic mode.

Keys are read from the environment and are never returned by the API or shown in the UI.

The dashboard opens on **Relux Home**, featuring a dynamic first-run checklist to guide initial setup, direct action links to key sections, and an overview of installed plugins. From there, you can chat with **Prime** (`POST /v1/relux/prime`), which now includes an action strip with practical example prompts for creating tasks, agents, and assigning work. Manage your **work** (tasks and runs) on the dedicated Work page, which offers a clear assignment/run workflow, conditional "Run assigned" actions, and task filtering. The **crew** (agents) page allows you to create and manage agents, with each agent's card linking directly to their assigned tasks. You can also install **plugins** and manage **approvals** and permissions – all backed by the local `/v1/relux` API, with no dependency on the legacy Relix bridge. Delegated tasks can be run by their assigned agent through the Work page, the API, or the CLI.
The served bundle is the committed build under
`crates/relix-web-bridge/dashboard-dist` (rebuild with `npm run build` in
`apps/dashboard`). See [`docs/RELUX_MASTER_PLAN.md`](docs/RELUX_MASTER_PLAN.md)
section 22 for the full MVP boot guide.

---

Relix is a local mesh of peer processes for running AI agents safely on
your own machine. Every call between peers carries a signed identity
bundle, passes a policy check, and writes a hash-chained audit record
before any handler runs. Orchestration lives in small flow files
(`.sol` or `.sflow`) whose only mesh primitive is
`remote_call(peer, method, args)`. There is no central gateway, no
shared credential store, no opaque tool registry — the HTTP bridge
that fronts OpenAI-compatible clients is just another peer.

You boot it with one command (`relix boot`), point any OpenAI-style
client at `http://127.0.0.1:19791/v1`, and you have a multi-agent
runtime with persistent memory, a tool surface, a scheduler, and an
operator dashboard.

**Quick links:**
[Getting started](docs/getting-started.md) ·
[Architecture](docs/architecture.md) ·
[Configuration](docs/configuration.md) ·
[Channels](docs/channels/index.md) ·
[Plugins](docs/plugins.md) ·
[SOL & Sflow](docs/sol.md) ·
[Changelog](CHANGELOG.md)

## Install

The default install gives you the latest **stable** release.

**Mac / Linux:**

```sh
curl -fsSL https://raw.githubusercontent.com/itsramananshul/Relix/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/itsramananshul/Relix/main/install.ps1 | iex
```

Both installers do the same four things:

1. Drop `relix`, `relix-controller`, and `relix-web-bridge` into
   `~/.local/bin` (or `RELIX_INSTALL_DIR`) and put it on your
   `PATH`.
2. Drop the mesh boot scripts into `~/.local/scripts/`.
3. Run **`relix setup`** automatically — the guided wizard:
   pick provider, paste API key, tick channels, confirm.
4. Save your choices to `~/.relix/config.toml`.

### Install a beta (pre-release)

`RELIX_CHANNEL=beta` installs the newest pre-release instead of the
latest stable. Betas are GitHub pre-releases — never shown as "Latest"
— so the stable install above is unaffected.

**Mac / Linux:**

```sh
curl -fsSL https://raw.githubusercontent.com/itsramananshul/Relix/main/install.sh | RELIX_CHANNEL=beta bash
```

**Windows (PowerShell):**

```powershell
$env:RELIX_CHANNEL = 'beta'; irm https://raw.githubusercontent.com/itsramananshul/Relix/main/install.ps1 | iex
```

### Pin an exact version

`RELIX_VERSION` installs one specific tag — stable (`v0.4.2`) or beta
(`v0.4.3-beta.1`) — and always wins over the channel.

**Mac / Linux:**

```sh
curl -fsSL https://raw.githubusercontent.com/itsramananshul/Relix/main/install.sh | RELIX_VERSION=v0.4.3-beta.1 bash
```

**Windows (PowerShell):**

```powershell
$env:RELIX_VERSION = 'v0.4.3-beta.1'; irm https://raw.githubusercontent.com/itsramananshul/Relix/main/install.ps1 | iex
```

The same per-OS binaries are built for both channels (Linux x86_64/arm64,
macOS x86_64/arm64, Windows x86_64); the installer auto-detects yours.
See [docs/releasing.md](docs/releasing.md) for how channels are cut.

## Quick start

After install, the wizard has already saved your config. Boot
the mesh:

```sh
relix boot                      # reads ~/.relix/config.toml
                                # opens http://127.0.0.1:19791/dashboard

relix status                    # is it up? show the topology table.
relix stop                      # kill the controllers + bridge by name.

relix setup                     # re-run the wizard to change provider /
                                # rotate keys / add a channel.
```

No env vars to export. No `--provider` flag to repeat. No
`scripts/...` to run by hand. The wizard wrote your provider +
API key + channel selections to `~/.relix/config.toml`; `relix
boot` reads them every time.

Once the bridge is healthy you'll have:

| | |
|---|---|
| Dashboard       | http://127.0.0.1:19791/dashboard |
| OpenAI-compat   | `POST http://127.0.0.1:19791/v1/chat/completions` |
| WebSocket chat  | `ws://127.0.0.1:19791/ws/chat` |
| Health          | http://127.0.0.1:19791/health |
| Topology + caps | `GET /v1/topology`, `GET /v1/capabilities` |

Point any OpenAI-compatible client (the official Python SDK, Open
WebUI, LobeChat, Cursor, etc.) at the bridge:

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:19791/v1", api_key="unused")
client.chat.completions.create(
    model="relix-openrouter",       # or relix-openai, relix-mock, ...
    messages=[{"role": "user", "content": "hello"}],
)
```

The bridge's API-key header is ignored — the real provider key
lives only on the AI node, sourced from `~/.relix/config.toml`.
See [docs/configuration.md](docs/configuration.md) for the full
config reference, including the `[ai.memory_peer]` knobs for
automatic conversation-history injection and optional RAG over
the vector store.

## CLI overview

`relix` is the unified operator CLI. Key subcommands:

| Subcommand | What it does |
|---|---|
| `boot` / `stop` / `status` | Start / stop / inspect the local mesh |
| `setup` | Re-run the interactive wizard (provider, API key, channels) |
| `health` | Relux kernel health check; exits non-zero on critical issues |
| `doctor` | Bridge health check; exits 1 on any FAIL |
| `update` | Self-update to the latest GitHub release |
| `release readiness` | Print or run the local first-release gate |
| `identity` | Org keypair generation, bundle minting, session tokens |
| `ping` | Raw libp2p health check against any peer |
| `task` | Coordinator Task ledger — create, list, get, watch, retry, export |
| `ops` | Operator snapshot surface — dispatch stats, policy simulate, agent/cron/delegate/msg surfaces |
| `workflow` | Multi-agent workflow engine — list, run, validate, trace |
| `planning` | Planner + critic pipeline — plan, search, validate |
| `metrics` | Agent performance metrics — summary, alerts, cost, timeseries |
| `observe` | Live TUI observability dashboard |
| `pii` | PII detection stats and event history |
| `training` | Training pipeline — list, show, export, delete |
| `knowledge` | Knowledge-share — groups, share, broadcast, revoke |
| `confidence` | Confidence scorer — policies, history, reset |
| `credentials` | Credential vault — store, list, rotate, revoke, audit |
| `approval` | Approval delivery inspector |
| `email` | Email channel — send, status, test |
| `execution` | Transactional gateway — rollback, transaction, evidence |
| `flow-run` | Compile and run a `.sol` flow against a live mesh |
| `router` | Router node control plane — network summary, peers, sessions |
| `mcp` | MCP registry inspector |
| `sessions` | Two-sink session debugger |
| `models` | Provider + model inventory |

Run `relix --help` or `relix <subcommand> --help` for flags.

## What's in the mesh

| Node          | Default port | What it does                                                                |
|---------------|--------------|-----------------------------------------------------------------------------|
| `memory`      | 19711        | SQLite + FTS5 chat memory, vector embeddings, persistent agent memory       |
| `ai`          | 19712        | `ai.chat` / `ai.embed` — `mock` / `openai` / `anthropic` / `openrouter` / `xai` / `gemini` / `local` |
| `tool`        | 19713        | File system, web (SSRF-guarded), terminal, headless browser, MCP, PDF, text |
| `coordinator` | 19714        | Durable task ledger, delegation, agent-to-agent messaging, cron scheduler, approval gate |
| `telegram`    | 19715        | Telegram bot bridge (opt-in)                                                |
| `discord`     | 19716        | Discord bot bridge (opt-in)                                                 |
| `slack`       | 19717        | Slack bot bridge (opt-in)                                                   |
| `email`       | configurable | Email channel bridge — SMTP outbound + IMAP inbound (opt-in)               |
| `plugin_host` | 19718        | Loads subprocess plugins over `relix-plugin-v1` (opt-in)                   |
| `web-bridge`  | 19791        | HTTP front, OpenAI shim, dashboard                                          |

Every node is its own OS process with its own libp2p identity. Every
inbound call passes: **identity verify → policy admit → handler →
signed audit record**. No bypass switch.

## Highlights

- **Bring your own provider.** `ai.chat` routes to `mock`, `openai`,
  `openrouter`, `xai`, `anthropic`, `gemini`, or any
  OpenAI-compatible `local` endpoint (Ollama, vLLM). Provider keys
  live only on the AI node. See [docs/configuration.md](docs/configuration.md).
- **Vector memory built in.** `memory.embed` + `memory.search` give
  you cosine top-K over per-subject embeddings; defaults to mock 8-dim
  vectors, switches to OpenAI `text-embedding-3-small` by changing the
  AI provider. [docs/memory.md](docs/memory.md).
- **Durable agents.** Every flow gets a `Task` row in the
  coordinator's SQLite ledger with attempts, events, lineage, todos,
  and pause/resume/cancel/replay primitives. [docs/coordination.md](docs/coordination.md).
- **Scheduled work.** `cron.create` accepts cron expressions, duration
  intervals, or one-shot timestamps. [docs/scheduler.md](docs/scheduler.md).
- **Channels.** Telegram, Discord, Slack, and Email channels route
  through the same SOL chat flow as the HTTP bridge and persist
  conversations in `memory`. [docs/channels/index.md](docs/channels/index.md).
- **Multi-agent planning.** The coordinator's planning pipeline
  (planner + critic) decomposes natural-language specs into delegated
  sub-tasks. Inspect via `relix planning plan --spec "..."`.
- **Knowledge-share.** Agents can share observations peer-to-peer with
  Ed25519-bound provenance. Source trust is configured per public key
  (`[knowledge_trust]`).
- **Training pipeline.** Interaction recording → PII anonymisation →
  quality scoring → OpenAI-format export. Controlled via `[training]`.
- **Confidence and reasoning.** Per-method rolling confidence scores
  feed the judge + belief-state engine (`[confidence]`,
  `relix confidence history`).
- **Metrics, observability, and alerting.** SQLite metrics store, cost
  tracking by model, OTLP export, configurable alert thresholds
  (`[metrics]`, `relix metrics`, `relix observe`).
- **Credentials vault.** Encrypted at-rest credential store; JIT
  injection into tool args via `{{secret:<name>}}` placeholders
  (`[credentials]`, `relix credentials`).
- **Approval gate.** Per-method approval tokens (Ed25519-signed, TTL
  30–86400 s). `RELIX_APPROVAL_SIGNING_KEY` env var required.
  Standing approvals and out-of-band delivery channels supported.
- **Mesh PII gate.** Inline regex scan of every inbound arg before
  handler dispatch; actions: `block`, `redact` (default), `log_only`.
  Writes a separate `pii_events.sqlite` chronicle (`[mesh_pii]`,
  `relix pii`).
- **Tenant isolation.** Per-tenant policy files + per-tenant SQLite
  audit mirror. Query via `node.audit.tenant_recent`.
- **Plugins.** Any language that can speak HTTP can ship a plugin —
  declared in `plugin.toml`, loaded as a subprocess by `plugin_host`,
  callable from SOL and `.sflow` like any built-in. Python (stdlib
  only) and Rust SDK examples ship. [docs/plugins.md](docs/plugins.md).
- **Three flow formats.** `.sol` is a Rust-flavoured imperative DSL;
  `.sflow` is a line-oriented step DSL with `try/catch`, loops, and
  `${var}` interpolation; `.yml`/`.yaml` is a YAML frontend that
  lowers to SOL. [docs/sol.md](docs/sol.md).
- **MCP support.** Register external MCP servers as proxied
  capabilities via `tool.mcp.*`.
- **Operator console.** The `/dashboard` single-page app ships
  twenty-two panels: Overview, Tasks, Scheduled Jobs, Chat,
  Memory, Approvals, Skills, Sessions, Reasoning, Credentials,
  Identity, Cost & Metrics, Observability, Policy Denials,
  Multi-Tenant, Planning, Workflows, Email, Plugins, MCP Servers,
  Configuration, and Logs. No extra config. Mesh topology, health,
  cost, pending approvals, and recent activity roll up into the
  Overview panel; the task ledger, cron jobs, policy denials, and
  MCP servers each have their own panel, backed by `/v1/tasks`,
  `/v1/cron/jobs`, `/v1/policy/denials`, and `/v1/mcp/servers`.
  The **Mandates** page turns a high-level goal into a Brief tree and
  guides it through governance: propose + approve a strategy, plan the
  team, inspect readiness, approve/reject pending clearances, then
  orchestrate (plan-only / create / create + assign, with a dry-run
  preview) and watch progress — over the existing `mandate.orchestrate`
  engine. Governance is surfaced, not bypassed.
  First sign-in creates a single admin (username + Argon2id password).
  **Forgot the local admin password?** Run
  `scripts/relix-dashboard-admin-reset.ps1` (Windows) or
  `scripts/relix-dashboard-admin-reset.sh` (macOS/Linux) — or
  `relix-web-bridge reset-admin` directly — then restart the bridge.
  It rewrites only the admin credential (`~/.relix/dashboard-admin.json`),
  prints a fresh password, and is **local-only**: there is no
  remote/unauthenticated reset.
- **Maintenance & storage.** Settings → *Maintenance & storage* (or
  `GET /v1/maintenance/summary`) shows run-workspace disk usage + run/log
  counts + warnings. Prune old run workspaces safely with a dry-run preview
  (`POST /v1/maintenance/prune`, dry-run by default — never touches the repo
  or a running run). Every prune is recorded in a durable audit
  (`GET /v1/maintenance/audit`, shown as *Cleanup history*). Optional
  scheduled cleanup (`RELIX_MAINTENANCE_AUTOPRUNE_*`, off + dry-run by
  default) automates it on a timer. Back up / restore local state with
  `scripts/relix-local-backup.ps1` / `.sh`. See `docs/operations.md`.

## Security model

- Each node runs as its own OS process with its own Ed25519 identity
  bundle, signed by an org root key.
- Every inbound call is verified against the bundle's signature and
  the org root before any handler logic runs.
- A per-node `PolicyEngine` evaluates `[admit]` groups + per-method
  `[[rules]]`. Default-deny; nothing runs without a matching allow.
- Every responder appends a signed, hash-chained record to its
  `audit.log` — `relix-flow-inspect` reads them offline.
- AI provider keys live **only** in the `ai` node's local config. The
  HTTP bridge never sees them.
- The tool node enforces a jailed filesystem root, an SSRF-guarded
  web client, an allowlisted shell, and PDF / chunk-size caps.

Full threat model, audit format, and disclosure process in
[docs/security.md](docs/security.md) and [SECURITY.md](SECURITY.md).

## Architecture

```
                          ┌─────────────────────────────┐
   OpenAI client  ───────▶│ relix-web-bridge :19791     │
   (chat.completions)     │  POST /v1/chat/completions  │
                          │  GET  /dashboard            │
                          └──────┬──────────────────────┘
                                 │  libp2p (signed envelopes)
                                 │
        ┌────────────┬───────────┴───────────┬────────────┬────────────┐
        ▼            ▼                       ▼            ▼            ▼
   ┌─────────┐  ┌─────────┐             ┌─────────┐  ┌─────────┐  ┌──────────────┐
   │ memory  │  │   ai    │             │  tool   │  │ coord-  │  │ plugin_host  │
   │ :19711  │  │ :19712  │             │ :19713  │  │ inator  │  │ :19718       │
   │ SQLite  │  │ provider│             │ fs/web/ │  │ :19714  │  │ subprocess   │
   │ + FTS5  │  │ routing │             │ term/   │  │ task    │  │ plugins      │
   │ vectors │  │         │             │ browser │  │ ledger  │  │              │
   └─────────┘  └─────────┘             └─────────┘  └─────────┘  └──────────────┘

   Channels (opt-in, each its own peer):
   ┌──────────┐  ┌─────────┐  ┌────────┐
   │ telegram │  │ discord │  │ slack  │
   │  :19715  │  │ :19716  │  │ :19717 │
   └──────────┘  └─────────┘  └────────┘
```

Every line in that diagram is a libp2p stream running the same
admission pipeline. There's no in-process fake P2P, no shared memory
between nodes.

## From source

```sh
git clone https://github.com/itsramananshul/Relix.git
cd Relix
cargo build --workspace
relix boot                               # uses target/debug binaries
```

Tests:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The same three commands run in CI on Linux / macOS / Windows.

## Docs by goal

| I want to ...                                                              | Read                                          |
|---------------------------------------------------------------------------|-----------------------------------------------|
| Get the mesh booting in 5 minutes                                          | [docs/getting-started.md](docs/getting-started.md) |
| Understand how the pieces fit together                                     | [docs/architecture.md](docs/architecture.md)  |
| Know every config key and env var                                          | [docs/configuration.md](docs/configuration.md) |
| Understand the security posture                                            | [docs/security.md](docs/security.md)          |
| Wire an agent's identity + permissions                                     | [docs/agents.md](docs/agents.md)              |
| Use memory (chat history + vectors + persistent)                           | [docs/memory.md](docs/memory.md)              |
| Schedule recurring work                                                    | [docs/scheduler.md](docs/scheduler.md)        |
| Coordinate multiple agents (delegation, messaging, approvals)              | [docs/coordination.md](docs/coordination.md)  |
| Connect Telegram / Discord / Slack                                         | [docs/channels/index.md](docs/channels/index.md) |
| Write a plugin                                                             | [docs/plugins.md](docs/plugins.md)            |
| Write a flow                                                               | [docs/sol.md](docs/sol.md)                    |

## Vex

The amber-and-electric-blue spider-phoenix in the logo is Vex. It's
the Relix mascot — geometric web silk on the wings, eight angular
legs, a burning ember at the centre. The metaphor is web (mesh) +
phoenix (durable, restart-safe). It also doubles as a favicon.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). PRs without AI co-author
trailers, against the CI gates, with docs updated where behaviour
changes.

## License

Licensed under either of [MIT](LICENSE) or [Apache License,
Version 2.0](LICENSE-APACHE) at your option. The Cargo manifest
declares `license = "MIT OR Apache-2.0"` on every crate so the
SPDX dual-license shape that the rest of the Rust ecosystem
expects (`license-file` consumers, downstream packagers,
crates.io publish) sees a consistent answer.

Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.
rms or conditions.
