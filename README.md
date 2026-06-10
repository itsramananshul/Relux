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
  <a href="https://github.com/itsramananshul/Relux/releases"><img src="https://img.shields.io/github/v/release/itsramananshul/Relux?include_prereleases&amp;style=for-the-badge" alt="Release"></a>
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

It serves on `http://127.0.0.1:19891` by default. If that port is already taken
(often because Relux is already running), `serve` stops with a clear message and
the exact command to pick another port — set `RELUX_HTTP_ADDR=127.0.0.1:<port>`
for a source checkout, or use `.\Start-Relux.ps1 -Port <port>` with the bundle.

#### Run the packaged release (v0.1.4, no build needed)

Prefer a prebuilt Windows bundle over building from source? Grab the latest
[**Relux local release**](https://github.com/itsramananshul/Relux/releases) zip
(`relux-local-0.1.4-windows-x64.zip`), extract it, and launch it - no Rust, no npm:

```powershell
# inside the extracted relux-local-0.1.4-windows-x64 folder
powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Relux.ps1
# override the port if 19891 is taken:  .\Start-Relux.ps1 -Port 20000
```

Then, in your browser:

1. Open **http://127.0.0.1:19891/dashboard** (the launcher prints this URL).
2. Go to **Health → Prime Brain / AI Runtime** and click **"Use Claude CLI for
   Prime"**. If `claude` is not yet on your PATH the panel shows the exact install
   + sign-in step (`npm i -g @anthropic-ai/claude-code`, then run `claude` once to
   log in); install it, click **Refresh**, then **"Use Claude CLI for Prime"**
   again. No JSON or env-var editing is required for normal Claude setup.
3. Open **Prime** and chat - e.g. `create a task to summarize the README`. A
   greeting stays a greeting; an action creates real, kernel-grounded work. Each
   reply shows its source (`via Claude CLI` / `deterministic`).
4. Open **Work** to see the created task, assign it to an agent on **Crew**, and
   run it with **Run assigned**.

The bundle stores its data under `.\data\local.db` next to `Start-Relux.ps1`, so
it is fully self-contained and portable.

**Product path (first release):** real work runs through a coding-agent **adapter**
- the **Claude CLI** or the **Codex CLI** - driven by **Prime** and its tools. Set
up an adapter from the dashboard (Crew → Adapters) and, optionally, give Prime a
natural voice with an OpenRouter key (Health → Prime AI settings). The bundled
`relux-tools-echo` / `relux-tools-status` handlers are **internal dev/test tools**
that prove the loop end-to-end (they back the offline smoke); they are not the
recommended user path and are not surfaced as a "run echo" button in the product.

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

Two built-in tools ship today. They are **internal dev/test built-ins** - the
honest, deterministic floor that proves the kernel/permission/audit loop and backs
the offline smoke. They are **not** the recommended product path (that is the
Claude/Codex adapters above) and are intentionally not promoted in the UI:

- `relux-tools-echo` / `echo.say` - returns its input unchanged (dev/test only).
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
  executable status (grounded discovery; it never invents a tool). Internal
  dev/test fixtures like `echo` are **hidden** from this catalogue and from the
  dashboard Plugins/Tools lists, so they are never offered as a real ability.
- "give me a status summary" / "what is going on?" - consults
  `relux-tools-status/status.summary` and answers from the real output.
- (dev/test only) `echo.say` is still invokable by exact name for the offline
  smoke, but it is intentionally not surfaced or suggested anywhere in the product.

Prime stays honest: a plain "hey" never becomes a tool call, and a request to use
an installed-but-unimplemented tool (e.g. a GitHub ToolSet) is reported as
"installed and discoverable, but this local runtime cannot execute it yet" with
no fabricated output. A missing permission is surfaced, never bypassed.
**Arbitrary downloaded plugin runtime execution remains intentionally not
implemented.** The CLI prints the invoked tool and its output too:

```powershell
relux-kernel prime "what tools can you use?"
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

### Installing plugins without a manifest (safe metadata wrapper)

A Relux plugin normally ships a `relux-plugin.json` manifest. To make arbitrary
GitHub repos / local folders / zips installable, the install paths no longer hard
fail when no manifest is present: Relux **generates a safe wrapper manifest**
instead.

- The id is derived from the repo/folder/zip name and sanitized
  (`relux-plugin-<name>`), so a hostile name can never escape the install root or
  collide with a bundled id.
- The generated manifest declares **no tools and no permissions**, is marked
  `Unverified`, and is authored by a `relux (generated manifest)` sentinel. It is
  **metadata only**: it runs nothing.
- Relux **never infers tool commands from repo content**. The plugin stays
  non-executable until you either configure an HTTP loopback runtime for it or add
  real tool definitions.
- An ambiguous source (more than one real plugin folder inside) is still a hard
  error rather than a silent guess.

On the dashboard, a generated wrapper is honest and actionable, not a dead-end
with a confusing badge:

- It is badged **Needs configuration** (never the green "enabled" a real plugin
  shows) and labelled a **Metadata-only wrapper** in the Kind column.
- Its next step is **Set up → add tool definitions**, *not* "configure a runtime".
  A wrapper declares no tools, and `discover_tools` only surfaces manifest-declared
  tools, so a loopback runtime alone would surface nothing. The Set up panel says
  this plainly and hands you a ready-to-edit `relux-plugin.json` (copy or
  download), keyed to the plugin id, plus the exact install directory and the
  three-step path: add the manifest → re-install → point a loopback runtime at a
  local server.
- After any install, the panel shows a **result summary**: tools discovered
  (count), a wrapper generated (nothing runnable yet), or an adapter installed —
  with the exact next step.
- The Tools list shows **only runnable tools by default**, with a "Show N
  non-runnable" toggle; a metadata-only plugin therefore never produces a
  ready-looking tool.

`/v1/relux/plugins` exposes a `generated: true` flag and a `tool_count` for each
record, and a read-only template endpoint backs the Set up affordance:

```text
GET /v1/relux/plugins/:id/manifest-template
  # { plugin_id, filename, install_dir, generated, manifest_json }
```

The `manifest_json` is a complete, re-installable starter `relux-plugin.json`
(ToolSet, one example tool, permission strings bound to this plugin id). It is
guidance only — Relux stores nothing from it and still runs nothing until you add
real tools and a runtime.

### Adapter Runtime v1 (local coding-agent CLIs)

An **Adapter** plugin decides how an assigned task runs. The bundled
`relux-adapter-local-prime` runs the deterministic echo path. Adapter Runtime v1
adds bundled adapters that drive a **local coding-agent CLI** you already have
installed:

- `relux-adapter-claude-cli` &rarr; runs `claude -p --permission-mode default
  --output-format json` (the JSON envelope is parsed into an honest summary + cost/
  usage; it is not a bypass/danger flag)
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

### Adapter run depth (what a run records, where to see it, retry)

Every adapter run is recorded so you can understand and recover it after the fact
&mdash; nothing shown is fabricated; it all comes from the durable transcript.

What a run records:

- A **lifecycle transcript**: `run_started` &rarr; `adapter_spawn` &rarr;
  `adapter_output` &rarr; `run_completed` / `run_failed`, each a durable, redacted,
  capped event.
- A **real measured `duration_ms`** (the actual wall time of the subprocess) and an
  honest status with a clear **failure reason** when it fails.
- The adapter's **(redacted, capped) stdout/stderr** on the `adapter_output` event,
  plus a bounded **output excerpt** on the run header. Secrets are scrubbed and big
  logs are capped &mdash; no secrets and no unbounded logs are ever stored.
- **Structured metrics when the CLI reports them.** The Claude adapter runs with
  `--output-format json`; Relux parses that result envelope into a human summary
  plus `usage` and `cost`, and treats an envelope `is_error` as a failure *even on
  a clean exit*. Codex and generic commands surface their plain text honestly (no
  invented metrics).

Where to see it:

- **Dashboard &rarr; Work &rarr; Recent Runs &rarr; Inspect.** The Run Detail panel
  shows the adapter, status, current/last phase, real duration, cost/usage (when
  present), the output excerpt, the failure reason, and the full transcript. A panel
  left open during a long run polls and refreshes the *real* recorded state (there
  is no fake/streamed progress yet).
- **API:** `GET /v1/relux/runs`, `GET /v1/relux/runs/:id` (carries the derived
  `phase` / `duration_ms` / `output_excerpt` / `failure_reason` / `retryable` /
  `cost` / `usage`), and `GET /v1/relux/runs/:id/events`.

Retry a failed run:

- A failed run is **retryable** as a *fresh* run on the **same task** &mdash; this is
  a new attempt, not a resume of a partial CLI run. It re-runs through the same safe
  gating (enabled runtime, binary on PATH, permission check) and records its lineage
  (`retried_from`).
- **Dashboard:** the Run Detail panel shows a **Retry** button on a failed run.
- **API:** `POST /v1/relux/runs/:id/retry` &rarr; `{ "run_id": "..." }`.
- **CLI:** `relux-kernel task retry-run <run_id>`.

Caveats: execution is synchronous (no live event streaming yet, so the UI
polls/refreshes); only the Claude adapter emits structured metrics today; a retry
is a new attempt rather than a resume.

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
# Quick gate: build dashboard, test + lint core/kernel, build the release binary,
# run doctor, and smoke Prime task creation + assigned-task execution.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-first-release-check.ps1

# Full product gate: everything above PLUS the standalone end-to-end smoke.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-first-release-check.ps1 -FullE2E
```

#### Standalone end-to-end smoke

`scripts\relux-e2e-smoke.ps1` proves the first version of the standalone Relux
product is actually usable - not just unit-tested - by driving the release
binary through every critical local flow against a **throwaway temporary
`RELUX_DB`** (it never touches your real `dev-data\relux\local.db` or any real
`serve` instance). Run it directly any time after a big chunk lands:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-e2e-smoke.ps1
```

It records PASS/FAIL/SKIP for each flow and proves:

- **doctor** reports healthy and the bundled plugin/adapter count includes every
  shipped bundle (echo, status, local-prime, claude-cli, codex-cli).
- **Prime chat**: a greeting creates no work and calls no tool; "what tools can
  you use?" lists the real built-in tools; a status question invokes the status
  tool; an echo request invokes the echo tool and returns the input.
- **Tool CLI**: `tools` lists the built-ins as ready; `tool invoke
  relux-tools-echo echo.say {json}` returns the same JSON.
- **Plugin Runtime v1 (HTTP loopback)**: it installs a temporary non-bundled
  ToolSet plugin, points it at a tiny loopback HTTP server the script runs
  itself, grants Prime its permission, invokes it through the kernel, and
  confirms the loopback server's output flowed back (`-SkipLoopback` to skip).
- **Adapter runtime controls**: `adapters` shows the claude/codex/local-prime
  records; enabling an adapter with a deliberately fake command persists and
  reports the runtime config, then disabling clears it - **no real Claude/Codex
  is ever spawned**.
- **Autonomy**: it creates a ready task through Prime, enables autonomy with safe
  settings, runs one tick, and verifies the task honestly moved Queued ->
  Completed with a run.
- **HTTP serve**: it starts `relux-kernel serve` on a free loopback port, hits
  `/dashboard`, `/v1/relux/state`, `/v1/relux/prime/autonomy`, and
  `/v1/relux/tools`, then stops the server (`-SkipServe` to skip).

Flags: `-SkipBuild` (reuse the existing release binary), `-SkipServe`,
`-SkipLoopback`, `-KeepTemp`. The script always cleans up its temp DB, server,
jobs, and processes, and exits non-zero on any failure.

Create a portable local bundle:

```powershell
# Quick package: run the quick readiness gate, then package.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1

# Full verified package: run the quick gate PLUS the standalone end-to-end smoke,
# then package. Use this when cutting a release candidate to share.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1 -FullE2E

# Fast repackage with no gate (still builds the release binary if missing).
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1 -SkipChecks
```

The package script writes `dist\relux-local-<version>-windows-x64\` and a zip
next to it (`dist\` stays gitignored and is never committed). The bundle is
self-describing:

- `relux-kernel.exe` - the Relux control-plane binary.
- `dashboard-dist\` - the built dashboard served at `/dashboard`.
- `examples\relux-plugins\` - the bundled example plugins/adapters.
- `docs\RELUX_MASTER_PLAN.md` and `README.md` - the design plan + reference.
- `Start-Relux.ps1` - a robust launcher that sets `RELUX_HTTP_ADDR`, `RELUX_DB`,
  and `RELUX_DASHBOARD_DIST`, prints the dashboard URL, and fails clearly if
  `relux-kernel.exe` is missing (`-Port` overrides the default 19891). It also
  preflights the port: if `127.0.0.1:<port>` is already in use it stops before
  launching (instead of printing a dashboard URL that points at the other
  process) and tells you to open the running instance or re-run with
  `-Port <free port>`.
- `VERSION.txt` + `RELEASE-NOTES.txt` - release metadata: version, git commit,
  build timestamp (UTC), the verification mode used (full e2e / quick / skipped),
  and the supported core loops (Prime chat, Work/task run, plugins, loopback tool
  runtime, adapter runtime controls, autonomy).

Run the bundle:

```powershell
cd dist\relux-local-<version>-windows-x64
powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Relux.ps1
# then open http://127.0.0.1:19891/dashboard
```

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

### Multi-agent orchestration (Prime as orchestrator)

When a goal clearly spans multiple roles, Prime can split it into **briefs
assigned to different agents** and run them as a **governed multi-agent batch** —
not just one local task at a time. Planning is deterministic and conservative: a
goal that does not split into at least two briefs is treated as a single task (no
storm). Creating an orchestration mints one brief per step and assigns each to a
fitting agent (a specialist on your roster, or Prime as a safe fallback); it does
**not** run anything until you start it. When obvious roles co-occur the planner
also infers **simple dependencies** — implementation waits on research, and
testing/review/documentation wait on implementation — so dependent work runs in
order. (Dependencies only ever point at earlier briefs, so the plan is always a
DAG; a goal without co-occurring roles has no dependencies and runs as before.)

```powershell
relux-kernel prime orchestrate "research the options, implement a prototype, and write the docs"
relux-kernel prime orchestration list
relux-kernel prime orchestration show <id>
relux-kernel prime orchestration run <id> [--max N] [--concurrency N]
```

Running a brief goes through **its assigned agent's own adapter**: a local agent
echoes deterministically, while an agent on an **enabled** Claude/Codex CLI adapter
runs the real CLI. The run loop is a **dependency-gated, round-based scheduler**: it
runs only **ready** briefs (every dependency completed), groups independent ready
briefs into **rounds bounded by a concurrency cap** (`--concurrency`, default 2,
max 4), and **honestly blocks** a brief whose dependency failed (never runs or fakes
it). A brief whose adapter runtime is disabled (or that is missing a permission) is
likewise recorded as **blocked** — never faked — with the exact next step. The
batch is bounded (`--max`), runs each brief once, records each brief's
start/finish + round, and stops safely. The Prime page has an **Orchestration**
panel (goal → preview plan with dependencies → create → run/continue, showing the
ready/waiting/blocked readiness, per-agent briefs, and the round each ran in), and
Home surfaces the newest unfinished orchestration with its next action. Every
orchestration is a durable, auditable trace of **goal → brief → agent → run**.

Runs are **non-blocking and pollable**: **Run/Continue** starts a background job
(`POST …/orchestrations/:id/run-async`, returning a job id + `status_url`) and the
panel polls it (`GET /v1/relux/orchestration-jobs/:job_id`) about once a second,
showing the live phase (Queued → Running — round N → Completed/Failed), a running
tally, and a real **running** badge on the brief(s) executing this round. The worker
drives the same governed scheduler one round at a time, **persisting progress
between rounds**, so the live view is recorded truth — nothing is faked. A second
start while a job is active is refused (one job per orchestration), and the button
is disabled while a job runs. The job registry is in-memory, so a server restart
mid-job is reported honestly (the poll 404s and the UI falls back to the durable
record, which still shows the rounds that actually completed).

*Honest limit:* briefs **within** a round currently execute sequentially through the
kernel's single-owner lock (the cap bounds round size; OS-parallel CLI spawns are a
later slice).

The background autonomy timer above is unchanged (deterministic, echo-only, never a
paid CLI); orchestration runs are operator-triggered from the UI, CLI, or API.

### Prime Brain (who answers Prime's chat)

Prime's **conversational** replies can come from one of four "brains", chosen from
**Health → Prime Brain / AI Runtime**. Prime's **actions** (creating tasks,
starting runs, approvals) always stay deterministic and kernel-grounded no matter
which brain is selected:

- **Local** (default) - the grounded, rule-based operator. Always available; no
  external call.
- **OpenRouter** - shape replies with an OpenRouter model (needs an API key; see
  below).
- **Claude CLI** - delegate replies to your local `claude` CLI (uses your Claude
  login; no key stored in Relux).
- **Codex CLI** - delegate replies to your local `codex` CLI (uses your ChatGPT
  login; no key stored in Relux).

For the CLI brains, the panel shows the live adapter status (installed/on-PATH,
enabled/disabled) and a one-click **"Use Claude/Codex for Prime"** that enables the
adapter and selects it as the brain. If the chosen CLI is missing or disabled, the
chat shows a clear, actionable note (with the exact next step) and falls back to the
grounded reply - never a blank page or a fabricated answer. Every Prime reply shows
which provider produced it (`via Claude CLI` / `via OpenRouter` / `deterministic`).
The brain is stored in the same local `ai-config.json`; the API is
`PUT /v1/relux/ai/config { "brain": "claude_cli" }` (values: `local` | `openrouter`
| `claude_cli` | `codex_cli`).

CLI brains are spawned the same safe way as assigned runs: argv-only, prompt on
stdin, a wall-clock timeout, an output cap, and secret redaction. Claude is invoked
in `--permission-mode default` (never `--dangerously-skip-permissions`).

#### Optional LLM-backed Prime (OpenRouter)

The OpenRouter brain enables a natural, LLM-backed chat path. Conversational
replies (greetings, status, explanations) are shaped by the model while actions
stay grounded and deterministic in the kernel.

**From the dashboard (recommended; no env vars).** Open **Health → Prime AI
settings**, paste your OpenRouter key (and optionally a model), and save. The key
is stored in a local, gitignored secrets file under the data root
(`<data-root>/ai-config.json`, e.g. `dev-data/relux/ai-config.json`) at `0600` on
Unix. It is **never** returned by the API or shown in the UI - only the key-free
status (`mode` / `configured` / `model`) is exposed. `relux-kernel serve` picks the
key up live, so no restart is needed. The API behind it:

```text
GET    /v1/relux/ai/status     # key-free: mode / brain / configured / model / reason
PUT    /v1/relux/ai/config     # { "provider":"openrouter", "api_key":"...", "model"?, "disabled"?, "brain"? }
DELETE /v1/relux/ai/config     # clear the stored key/config
```

Only **OpenRouter** takes an API key. **Claude and Codex are run as adapters** and
authenticate through their own local CLI login - there is no key to paste for them.

**From the environment (CLI-only setups still work).**

1. Set `RELUX_OPENROUTER_API_KEY` to your OpenRouter key.
2. (Optional) Set `RELUX_OPENROUTER_MODEL` (default: `openai/gpt-4o-mini`).
3. (Optional) Set `RELUX_LLM_DISABLED=1` to force deterministic mode.

A key in the dashboard secrets file wins per-field; any field it omits falls back
to the environment. Keys are never returned by the API or shown in the UI.

The dashboard opens on **Relux Home**, featuring a dynamic first-run checklist to guide initial setup, direct action links to key sections, and an overview of installed plugins. From there, you can chat with **Prime** (`POST /v1/relux/prime`), which now includes an action strip with practical example prompts for creating tasks, agents, and assigning work. Manage your **work** (tasks and runs) on the dedicated Work page, which offers a clear assignment/run workflow, conditional "Run assigned" actions, and task filtering. The **crew** (agents) page allows you to create and manage agents, with each agent's card linking directly to their assigned tasks. You can also install **plugins**, manage **approvals** and permissions, and check **health** — a local readiness surface with state counts, plugin/tool/adapter status, Prime autonomy status, and the package/check command hints. All of these are backed by the local `/v1/relux` API, with no dependency on the legacy Relix bridge. The old bridge-backed Relix pages are not part of this standalone shell and do not appear in its navigation; they remain reachable only at their legacy paths. Delegated tasks can be run by their assigned agent through the Work page, the API, or the CLI.
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
