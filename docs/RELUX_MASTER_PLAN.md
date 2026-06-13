# Relux Master Plan

Version: 0.1.1
Status: Canonical planning document
Product name: Relux
Repository: `D:\DATA\WORK\OpenPrem\Apps\Relix-Revised`
Remote: `https://github.com/itsramananshul/Relux`

This document exists so Codex, Claude, and any future AI working in this
repository understand what Relux is, why the direction changed, what must be
built, what must not be built, and how the current Relix codebase should be
transformed into the new Relux product.

Read this document before doing product, architecture, backend, frontend,
dashboard, plugin, agent, or Prime work.

Do not treat older Relix docs as deleted knowledge. They contain valuable
implementation and product lessons. But when there is a conflict, this master
plan defines the current product direction.

---

## 1. The Core Decision

Relux is not only an AI company.

Relux is not only a plugin manager.

Relux is not only a chatbot.

Relux is not only a workflow runner.

Relux is a Prime-centered agentic control plane.

Relux feels like Codex, but with more structure, more memory, more visibility,
more permissions, more plugins, and the ability to create or coordinate other
agents.

The user should be able to talk to Prime the way they talk to Codex:

```text
hey
what is going on?
build this
continue that
run Claude on this
hire another coding agent
give this agent GitHub read access only
show me active runs
why is this blocked?
retry the failed run
turn this into a company
```

Prime should understand the user, inspect state, take allowed actions, ask for
approval when needed, create tasks, start runs, call plugins, and coordinate
agents.

The product must preserve the things the user liked from the old Relix and
Paperclip direction:

- an issue/task board
- active runs
- task detail pages
- transcripts
- agents
- ability to spawn/hire more agents
- permissions
- approvals
- audit logs
- dashboard visibility
- work that can continue over time
- product feeling like an actual app, not a pile of endpoints

But the product must become more flexible than a fixed company metaphor.

The company structure is a template/mode, not the whole identity.

Relux can be:

- a company of agents
- a coding workspace
- a research lab
- a support automation hub
- an enterprise internal agent network
- a personal AI operating system
- a plugin-powered workflow runner
- a team of specialist agents
- a control plane for agentic applications

The same core system supports all of these.

The short version:

```text
Relux = Codex-like Prime + task board + active runs + agents + plugins +
permissions + approvals + audit + dashboard.
```

---

## 2. The Product North Star

Relux is the control plane for building and operating agentic systems.

At the center is Prime.

Prime is the main intelligent operator. Prime is an LLM-backed agent with a
toolbelt, memory, state awareness, and authority limited by permissions.

Around Prime is the control plane:

- plugins
- agents
- tasks/issues
- runs
- permissions
- approvals
- audit logs
- secrets
- workspaces
- dashboard
- CLI
- APIs

Relux should make the user feel:

```text
I can tell Prime what I want.
Prime understands.
Prime can do work directly.
Prime can create tasks.
Prime can spawn or assign agents.
Prime can use tools through plugins.
Prime can show me what is happening.
Prime cannot silently do dangerous things.
Everything is visible, permissioned, and auditable.
```

The first successful version of Relux is not a massive marketplace. It is a
working Prime loop:

```text
User talks to Prime.
Prime creates or selects a task.
Prime starts a run or assigns an agent.
The run uses an adapter/tool plugin.
Permissions are checked.
Events are logged.
The dashboard shows the task, run, transcript, result, and audit trail.
Prime can summarize, retry, continue, or ask approval.
```

That loop is the foundation.

---

## 3. What Relux Is

Relux is:

1. A Prime-centered control plane for agentic applications.
2. A plugin-first kernel for models, tools, agents, memory, storage, and
   execution environments.
3. A task and run system for durable work.
4. A permissions and approval layer around agent actions.
5. A dashboard for understanding what Prime and agents are doing.
6. A CLI/API platform for developers building agentic systems.
7. A flexible operating layer that can become a company, lab, automation hub,
   coding workspace, or custom agent network.

Relux is designed to answer these questions:

```text
Who can act?
What can they access?
Which plugin powers this capability?
What task is being worked on?
Which run is active?
What tools were called?
What was denied?
What needs approval?
What failed?
What succeeded?
What should happen next?
```

---

## 4. What Relux Is Not

Relux is not a fixed "AI company" product only.

It can have a company template, but a company is one possible shape.

Relux is not just a plugin manager.

The plugin system is infrastructure. The user-facing product is Prime and the
operating surface around Prime.

Relux is not a generic chat app.

Chat is the main command surface, but the source of truth is durable state:
tasks, runs, agents, plugins, permissions, approvals, and audit logs.

Relux is not a random collection of panels.

Every dashboard screen must answer one of these:

- What is Prime doing?
- What are agents doing?
- What work exists?
- What is running?
- What needs approval?
- What can each actor access?
- Which plugins power the system?
- What happened?

Relux is not a system where an LLM can do anything by vibes.

Every meaningful action must pass through permissions, risk checks, and audit.

---

## 5. Why The Direction Changed

The old Relix direction had many strong ideas:

- secure mesh
- policy gates
- audit/Chronicle
- Rigs/adapters
- Prime
- agents/operatives
- Briefs
- active runs
- dashboard
- budget/allowance
- approvals/clearances

But the old product felt too much like infrastructure and too little like a
usable product. It had many capabilities, but the user had to connect them.

Paperclip felt better because its execution was product-path first:

- issue board
- issue detail
- chat/comments around an issue
- runs attached to issues
- agents assigned to work
- live logs/transcripts
- recovery states
- approvals
- a dashboard that made work legible

The user liked:

- the concept of Relix
- the execution discipline of Paperclip
- the feeling of Codex as a smart agent that can talk, act, and coordinate

Relux combines those lessons:

```text
Relix concept and security discipline
+ Paperclip-style work/run visibility
+ Codex-like Prime
+ plugin-first extensibility
= Relux
```

---

## 6. The Correct Mental Model

Prime is not a button.

Prime is not a dumb state machine.

Prime is not a generic chatbot that creates a plan from every message.

Prime is a Codex-like intelligent operator inside Relux.

Prime has:

- conversation ability
- state inspection
- tool/action ability
- permission awareness
- task creation ability
- run starting ability
- agent spawning/hiring ability
- delegation ability
- approval request ability
- explanation ability
- recovery/retry ability

Prime should behave like this:

```text
User: hey
Prime: I am here. There are 2 active runs and 1 task waiting for approval.
       What do you want to work on?

User: build a coding agent that can open PRs
Prime: I can do that. I need an adapter plugin, GitHub tool plugin, and
       scoped GitHub permissions. I can create the agent with read/create-PR
       permissions but not merge permission. Proceed?

User: yes
Prime: Created the agent, installed/configured the required plugins, granted
       scoped permissions, and created a test task.

User: why did the run fail?
Prime: The terminal tool failed because the workspace path was missing. I can
       retry after creating the workspace, or mark the task blocked.
```

Prime should not behave like this:

```text
User: hey
Prime: Here is a full strategy and 12 subtasks.
```

Prime must understand intent before acting.

---

## 7. Product Layers

Relux has six product layers.

### 7.1 Prime Layer

Prime is the main conversational and action-taking surface.

Prime can:

- chat with the user
- inspect system state
- create tasks
- update tasks
- start runs
- assign tasks to agents
- create/spawn/hire agents
- install/configure plugins
- grant/request permissions
- ask for approval
- summarize runs
- explain blockers
- retry or continue work

Prime must route actions through the kernel, not bypass it.

### 7.2 Work Layer

The work layer contains tasks/issues, boards, and task detail, including detailed views for individual tasks and runs.

Core objects:

- Task or Issue
- Run
- Active Run
- Artifact
- Comment/Event
- Approval
- Audit Event

The board is central. The user likes the issue board. Keep it.

The board should be understandable:

```text
Backlog
Ready
Running
Waiting Approval
Blocked
Done
Failed
```

Names can evolve, but the statuses must be obvious.

### 7.3 Agent Layer

Agents are configured actors inside Relux.

An agent has:

- name
- role/purpose
- adapter plugin
- model/runtime config
- permissions
- available tools
- memory settings
- task queue
- active run state
- owner/namespace
- audit history

Prime may act directly, or Prime may create/assign work to agents.

Agents are not necessarily "employees" unless the selected template is a
company template. In generic Relux, they are specialist actors.

### 7.4 Plugin Kernel Layer

The plugin kernel owns:

- plugin discovery
- plugin installation
- plugin manifests
- plugin registry/local index
- plugin enable/disable
- plugin health
- plugin routing
- plugin permissions
- plugin audit

Plugins provide:

- adapters
- tools
- service providers
- memory providers
- vector stores
- execution environments
- task brokers
- UI panels
- company/internal integrations

### 7.5 Permission And Approval Layer

Every meaningful action is permissioned.

Examples:

```text
tool:relux-tools-github:create_pr
tool:relux-tools-github:merge_pr
tool:relux-tools-terminal:run_tests
exec:relux-env-python-wasm:run
plugin:relux-tools-github:configure
agent:code-agent:assign_task
task:task_123:start_run
```

Some actions require approval:

- merging PRs
- deleting files
- changing production systems
- sending external messages
- issuing refunds
- reading sensitive data
- running destructive shell commands
- granting broad permissions

Prime and agents can request approval. They cannot silently bypass it.

### 7.6 Dashboard Layer

The dashboard supports the product. It should not replace Prime.

Core dashboard pages:

- Prime Chat
- Board / Tasks
- Active Runs
- Task Detail
- Agents
- Plugins
- Permissions
- Approvals
- Audit Logs
- Settings

The dashboard must feel like a real product, not generated placeholder UI.

---

## 8. Plugin Model

Everything important should eventually be plugin-powered.

### 8.1 Adapter Plugins

Adapter plugins connect Relux to models or agent runtimes.

Examples:

- `relux-adapter-openai`
- `relux-adapter-anthropic`
- `relux-adapter-openrouter`
- `relux-adapter-claude-cli`
- `relux-adapter-codex-cli`
- `relux-adapter-hermes`
- `relux-adapter-ollama`
- `relux-adapter-custom-http`

Adapters answer:

- How does this agent/model receive tasks?
- How does it stream events?
- How does it call tools?
- How does it report usage/cost?
- How does it resume?
- How does it fail?

**Canonical setup surface — Crew → Prime Brain.** The single canonical place an operator
configures Prime's brain (who answers chat) and its work adapters (Claude/Codex CLI) is the
shared `PrimeBrainPanel` mounted under **Crew → Prime Brain**, with adapter enable/disable on
**Crew → Adapters** (anchors `#prime-brain` / `#adapters`). All product guidance routes here:
the first-run readiness guide (`onboarding.ts`/`readiness.ts`), the kernel doctor's remediation
links (`doctor.rs`), and the local-prime external-work refusal (`KernelError::
LocalAdapterUnsupported`) all deep-link to `/crew#prime-brain` or `/crew#adapters` (never the
stale "Health → AI settings"). The **Health** page still mounts the same `PrimeBrainPanel` as a
**secondary diagnostics duplicate** — Health is the read-only doctor/health hub, so it mirrors
the panel for convenience while the doctor report it renders points at the canonical Crew anchor.
The local-prime refusal additionally names the **plugin-install alternative** (Plugins → +
Install → GitHub URL) when the requested work is "import a repository as a plugin".

### 8.2 ToolSet Plugins

ToolSet plugins expose tools.

Examples:

- `relux-tools-github`
- `relux-tools-terminal`
- `relux-tools-browser`
- `relux-tools-slack`
- `relux-tools-discord`
- `relux-tools-tavily`
- `relux-tools-google-drive`
- `relux-tools-zendesk`
- `relux-tools-salesforce`

Tools must declare:

- name
- description
- input schema
- output schema
- risk level
- required permission
- approval requirement
- timeout
- retry policy

### 8.3 Service Provider Plugins

Service providers replace infrastructure backends.

Examples:

- `relux-provider-sqlite`
- `relux-provider-postgres`
- `relux-provider-redis`
- `relux-provider-nats`
- `relux-provider-qdrant`
- `relux-provider-chromadb`
- `relux-provider-s3`
- `relux-provider-localfs`

Provider traits may include:

- PrimaryStorage
- VectorStore
- TaskBroker
- BlobStorage
- MemoryStore
- SecretStore
- EventBus

### 8.4 Execution Environment Plugins

Execution environments run code or programs.

Examples:

- `relux-env-python-wasm`
- `relux-env-node-wasm`
- `relux-env-docker`
- `relux-env-firecracker`
- `relux-env-sol`
- `relux-env-shell`
- `relux-env-browser`

They must declare:

- supported language/runtime
- resource limits
- network policy
- filesystem policy
- timeout
- isolation mode
- risk level

### 8.5 UI Plugins

Eventually, plugins may add dashboard panels or task detail cards.

This is not MVP-critical. Do not build this before the basic Prime/task/plugin
loop works.

---

## 9. Core Entities

The first Relux data model should be simple and durable.

### 9.1 User

A human or service actor.

Fields:

- id
- name
- email or handle
- role
- namespace memberships
- permissions
- created_at

### 9.2 Namespace

A scope for resources.

Examples:

- personal workspace
- company
- project
- team
- customer
- environment

Fields:

- id
- name
- kind
- parent namespace
- settings
- created_at

### 9.3 Agent

A configured agent actor.

Fields:

- id
- name
- description
- adapter plugin
- adapter config
- persona/instructions
- namespace
- owner
- permissions
- status
- created_at

### 9.4 Plugin

An installed plugin record.

Fields:

- id
- name
- version
- type
- manifest
- source
- trust level
- enabled
- health
- installed_at

### 9.5 Task

A durable unit of work.

Fields:

- id
- title
- input/body
- status
- priority
- created_by
- assigned_agent
- namespace
- required_permissions
- parent_task
- deadline
- created_at
- updated_at

Statuses:

```text
created
queued
leased
running
waiting_for_tool
waiting_for_approval
blocked
completed
failed
cancelled
expired
```

**Ad-hoc subtrees (`parent_task`).** `parent_task` is now **populated** for hand-made
subtasks (an operator breaking a task down outside any orchestration). The kernel's
`create_task_with_parent` validates the edge before persisting it — the parent must
exist, share the child's namespace, and not close a cycle (the bounded, cycle-guarded
task-tree walk in `relux_core::task`) — and `POST /v1/relux/tasks` accepts an optional
`parent_task`. This is the second real parent→child link beside the orchestration's
`steps[]`; the dashboard renders it as a subtree on the Work board
(`docs/relix-dashboard-design.md` §6.3). The edge can also be **safely reparented**
after creation via `KernelState::reparent_task` / `POST /v1/relux/tasks/:id/parent`
(move under a new parent or clear it) — the same exist/namespace/cycle validation,
structural only (status / agent / runs untouched); §6.6.

**Reopening blocked work is a lifecycle action, not a status decree.** The board's
operator status allowlist (§6.4) offers only `blocked` / `cancelled` — the
machine-driven lanes (`running`/`queued`/`completed`/…) are set by the run lifecycle,
never decreed from the board. So putting a **blocked** task back into the run lifecycle
is its own validated action, not a `set_task_status` call: `KernelState::reopen_task` /
`POST /v1/relux/tasks/:id/reopen` re-queues a blocked task (`Blocked` → `Queued`) after
validating it exists, is currently blocked (`TaskNotReopenable` → 409 otherwise), and
has an assigned operative (`TaskNotAssigned` otherwise — a run needs an assignee). It
touches no run and does not auto-execute; the existing **Run (Assigned)** path then runs
the re-queued task through the unchanged run gate. A one-click **Reopen & run**
(`KernelState::reopen_task` then `KernelState::execute_assigned_run`, exposed as
`POST /v1/relux/tasks/:id/reopen-and-run`) chains those two **existing** chokepoints in a
single governed call — same eligibility guard, same run gate, no bypass: an ineligible
task fails (4xx) before any run; a reopen that succeeds but whose run is honestly refused
returns 200 with `reopened: true` / `run_id: null` / a `run_refused` message and the
reopened state preserved. This is distinct from the run-level
**retry** (a fresh attempt of a failed *run*, §10.2) and **resume** (continuing a
captured Claude *session*, `POST /v1/relux/runs/:id/resume`) — reopen targets the
*task*, those target a *run*. A task waiting on an approval is not blocked, so it routes
to Approvals, not reopen. Dashboard surface: `docs/relix-dashboard-design.md` §6.9.

### 9.6 Run

One execution attempt for a task.

Fields:

- id
- task_id
- agent_id
- adapter_plugin
- status
- started_at
- ended_at
- usage
- cost
- summary
- error
- artifacts — read-only artifact **references** the adapter declared in its
  structured result envelope (`artifacts: [...]`): each a bounded, redacted,
  path-sanitized reference (name / type / summary / source, optional relative
  path + size). These are references, **not** a workspace diff or an apply plan,
  and capturing them does not enable apply (see section 15). Empty when the
  adapter declared none. Never fabricated.
- proposed_changes — reviewable, applyable **proposed file changes** the adapter
  declared in its structured result envelope (`proposed_changes: [...]`): each a
  bounded, path-sanitized, text-only change to one file — a full-content
  `replace`, a new-file `create`, a `rename`/move to a `dest_path`, or a `delete`
  (`path` / `action` / `dest_path?` / `new_content` / `baseline_sha256?` /
  `new_sha256` / `bytes` / `source`) with a review `status` (proposed → approved →
  applied, or rejected). Unlike
  `artifacts` (read-only references), these carry content and ARE the first real
  Relux diff/apply model: the operator reviews (approve / reject) and, once
  approved, explicitly applies into the run's controlled workspace root with a
  baseline-conflict check (see section 15). Empty when the adapter declared none.
  Never fabricated; apply is never automatic.
- last_activity_at — real wall-clock seconds (Unix epoch) of the run's most recent
  transcript activity, set when the run starts and bumped on every run event the
  kernel pushes. Distinct from `started_at`/`ended_at` (logical-clock strings for
  ordering, not wall time). It is the heartbeat the **run watchdog** reads (see
  section 9.6.1). Honest wall-clock, like `duration_ms`; omitted from the wire when
  absent (an older record).

#### 9.6.1 Run Lifecycle & Stall Recovery (the no-silent-hang guarantee)

A run is created in `running` the instant `start_run` commits it, and a real
execution — the deterministic local path, or an off-lock CLI spawn — is expected
to drive it to a terminal state (`completed` / `failed` / `cancelled`). Every run
MUST end in exactly one of four observable outcomes, never an indefinite silent
`running`:

1. **Progress** — visible transcript/log activity (adapter selected, command/brain
   path, tool calls, streamed output).
2. **Success** — `completed` with a visible result.
3. **Failure / blocked** — `failed` (with a structured `failure_class` +
   remediation) or the task moved to `blocked` with an actionable reason.
4. **Recoverable stale** — recovered by the watchdog with a visible
   retry / cancel / investigate path.

A few things can break the "execution drives it to terminal" expectation without
any error reaching the operator: a run that is started but never executed (a
dangling `start_run`), an off-lock adapter spawn whose process is killed (or the
whole kernel restarted) before `finalize_cli_run` records the outcome (an
**orphaned** run), or a background job that dies mid-flight. In every case the run
would otherwise sit in `running` forever with a transcript that stops after
`run_started`.

The **run watchdog** is the server-side backstop. A periodic sweep (in the always-on
background loop, every iteration regardless of whether Prime autonomy is enabled)
recovers any run that has been `running` with **no new transcript activity**
(`last_activity_at`) for longer than the configured stall window AND is **not**
currently live. "Live" = the run is streaming a log buffer or holds a live cancel
token (a genuinely-executing off-lock run); those are NEVER flagged, so a long,
quiet-but-real run (the model is thinking) is safe. A recovered run is marked
`failed` with `failure_class = stale`, gets a `run_stalled` transcript event, and
its task moves to `blocked` so it surfaces in oversight + the Inbox with the usual
recovery actions. A stall is NOT auto-retried (it is unexpected) — the operator
chooses retry / cancel / investigate; **Retry** re-queues and actually re-executes
through the assigned adapter.

This mirrors how the reference agents refuse to hang: Hermes caps a provider read
with a hard wall-clock timeout (`agent/anthropic_adapter.py` `_read_timeout`,
default 900s) and the openclaw/codex runtime carries a `stream_idle_timeout_ms`
inactivity abort (`extensions/acpx/src/codex-trust-config.ts`). Relux maps the same
"no progress ⇒ stop, never hang" idea onto its own run lifecycle.

The policy is operator-tunable and visible, never a hidden constant:
`GET`/`PUT /v1/relux/watchdog` → `{ enabled, stale_after_secs }`, surfaced on the
Health page (Run Watchdog panel). `stale_after_secs` is clamped to a safe band
(30s–6h; default 180s) on load and on update.

### 9.7 Run Event

A transcript/log/timeline event inside a run.

Fields:

- id
- run_id
- ts
- kind
- source
- message
- structured_payload

`GET /v1/relux/runs/:id/events` accepts an optional `?since=<event_id>` exclusive
cursor that returns only the events strictly after that id (the incremental
live-tail); absent/empty `since` returns the full transcript. The Work-page Run
Detail uses this to live-tail an in-flight run cheaply (fetch only the new tail,
merge by id) instead of re-fetching the whole transcript each poll. `ts` is a
logical-clock string (ordering, not wall time), so the dashboard's honest
"No activity for Xs" stalled-run signal is measured against real wall-clock
elapsed time in the client, never derived from `ts` — and it is never a
fabricated progress bar.

### 9.8 Tool Call

A tool invocation routed through the kernel.

Fields:

- id
- run_id
- agent_id
- plugin
- tool
- input
- output
- permission
- risk
- status
- approval_id

### 9.9 Approval

A human approval request.

Fields:

- id
- requested_by
- action
- reason
- risk
- status
- approved_by
- created_at
- resolved_at

### 9.10 Audit Event

Immutable record of important actions.

Fields:

- id
- ts
- actor
- action
- target
- namespace
- result
- metadata
- hash/chain metadata if supported

---

## 10. Prime Behavior Specification

Prime needs an intent layer, a planning layer, and an action layer.

### 10.1 Intent Layer

Prime should classify user messages before acting.

**Brain-mediated classification (implemented, post-v0.1.7).** The deterministic
keyword classifier (`crates/relux-kernel/src/prime.rs` `classify_intent`) is now a
*fallback safety rail*, not the primary brain. When a real brain is configured
(OpenRouter or a local Claude/Codex CLI) it *proposes* the intent through a
structured, JSON-only decision stage (`crates/relux-kernel/src/prime_intent.rs`):
the proposed label is validated against the `PrimeIntent` allowlist, then
reconciled through a **fail-closed gate** (`reconcile_intent`) that preserves the
Conversation Rules (§10.5) and §17.1 — guarded chat (ideation/questions without an
explicit command) can **never** be promoted to a work intent, a low-confidence
proposal keeps the deterministic intent, and a `create_and_run` without explicit
run language is downgraded to `create`. The brain decides *intent only*; slots and
durable actions still flow through `decide` → `prime_execute`. Any brain failure
falls back to the deterministic classifier. This is the rule in
`docs/reference-driven-development.md` applied: the shape is taken from Hermes'
allowlist-validated tool loop and Paperclip/openclaw's fail-closed mutation gate.

Intent categories:

- greeting
- status question
- task creation
- task update
- run start
- run retry
- agent creation
- plugin installation
- permission change
- approval response
- explanation request
- dashboard/navigation request
- brainstorming
- direct answer/no action

Examples:

```text
"hey" -> greeting
"what is running?" -> status question
"fix this bug" -> task creation
"start it" -> run start, based on current context
"hire a browser agent" -> agent creation
"give it GitHub access" -> permission change, probably approval
"why did it fail?" -> explanation request
```

### 10.2 Action Layer

Prime can call kernel actions.

Initial Prime actions:

```text
prime.inspect_state
prime.create_task
prime.update_task
prime.assign_task
prime.start_run
prime.retry_run
prime.create_agent
prime.install_plugin
prime.install_plugin_from_github   # import a GitHub repo as a plugin (confirm-gated; metadata only, no code run)
prime.configure_plugin
prime.grant_permission
prime.request_approval
prime.summarize_run
prime.explain_blocker
```

Prime must never bypass the kernel.

### 10.3 Approval Rules

Prime may propose risky actions. Prime may request approval. Prime may explain
why an action is needed.

Prime must not silently:

- grant broad permissions
- run destructive tools
- expose secrets
- merge/deploy/delete
- read sensitive data
- install untrusted plugins
- change production config

### 10.4 Delegation Rules

Prime can do work directly or delegate.

Prime should delegate when:

- the task is long-running
- the task requires a specialist agent
- multiple tasks can run in parallel
- the user asks to hire/spawn/assign
- a template says this work belongs to a specific role

Prime should act directly when:

- the user asks a simple question
- the user wants a status summary
- the action is small and within Prime's permissions
- the system needs coordination or explanation

### 10.5 Conversation Rules

Prime should be natural, but not reckless.

Prime should:

- answer greetings normally
- ask clarifying questions when needed
- show what it is doing
- explain why approval is needed
- summarize results
- keep state grounded in tasks/runs/plugins

Prime should not:

- create plans from casual greetings
- invent completed work
- hide failures
- pretend plugins exist when they do not
- silently perform dangerous actions

---

## 11. Dashboard Product Spec

The dashboard should be operational, dense, and usable.

It should not look like a marketing page.

It should not be a pile of disconnected feature panels.

It should be centered on Prime, work, runs, agents, plugins, and permissions.

### 11.1 Prime Chat

The main page or primary surface.

It shows:

- chat with Prime
- Prime suggested next actions
- current context
- active tasks/runs summary
- approval prompts
- plugin/action results

### 11.2 Board

The board is core.

Columns should be simple:

- Backlog
- Ready
- Running
- Waiting Approval
- Blocked
- Done
- Failed

Each card should show:

- title
- assigned agent
- status
- latest run
- blockers
- approval needed
- plugin/tool summary

### 11.3 Active Runs

Shows live or recent runs.

Each run should show:

- task
- agent
- adapter
- status
- duration
- tool calls
- transcript snippet
- cancel/retry/details actions

### 11.4 Task Detail

Task detail is where work becomes legible.

It should show:

- task title/input
- assigned agent
- status
- comments/events
- active run
- run transcript
- tool calls
- approvals
- artifacts/output
- audit trail

### 11.5 Agents

Shows configured agents.

Each agent should show:

- adapter plugin
- status
- current task/run
- permissions
- tools
- memory config
- risk profile

### 11.6 Plugins

Shows installed plugins.

Each plugin should show:

- type
- version
- status
- health
- capabilities exposed
- permissions exposed
- configuration state

### 11.7 Permissions

Shows user/agent/plugin permissions.

Must support:

- searching permissions
- granting/revoking
- risk warnings
- approval rules
- permission templates

### 11.8 Approvals

Shows pending human approvals.

Each approval should show:

- requester
- action
- risk
- reason
- target
- approve/reject/ask changes

### 11.9 Audit Logs

Searchable action history, now accessible via `GET /v1/relux/audit?limit=N` (default 100, max 500).

Shows:

- actor
- action
- target
- result
- plugin
- task/run
- timestamp
- namespace

---

## 12. CLI Product Spec

The CLI should make Relux feel easy to start.

Ideal flow:

```bash
relux init
relux plugins install relux-adapter-anthropic
relux plugins install relux-tools-terminal
relux agents create code-agent --adapter relux-adapter-anthropic
relux permissions grant code-agent tool:relux-tools-terminal:run_tests
relux up
relux tasks create --agent code-agent --title "Run tests" --input "Run the test suite and summarize failures."
relux tasks watch task_123
```

Eventually:

```bash
relux prime chat
relux plugins search github
relux plugins install relux-tools-github
relux plugins configure relux-tools-github
relux agents create
relux tasks board
relux runs list
relux approvals list
relux audit search
```

The CLI must be useful, but the dashboard and Prime chat should be the primary
human operating surfaces.

---

## 13. Relationship To Existing Relix Code

The current repository is a revised copy of the old Relix codebase.

Do not blindly delete old systems.

Do not blindly keep old names forever either.

Old systems should be mapped into the new Relux product model.

### 13.1 Naming Map

Product-facing language should gradually move toward Relux terms.

```text
Relix -> Relux
Rig -> Adapter plugin
Operative -> Agent
Brief -> Task or Issue
Shift -> Run
Chronicle -> Audit log or Timeline
Clearance -> Approval
Key -> Permission
Guild/Company -> Namespace or Template
Mandate -> Project/Goal/Task group, depending on context
The Desk -> Dashboard
```

Internal Rust names do not all need to change immediately. Product-facing UI,
docs, and APIs should move first. Internal renames should happen only when safe
and useful.

### 13.2 What To Keep

Keep or adapt:

- run ledger
- active runs
- task/brief store
- dashboard auth
- adapter/Rig code
- policy/permission ideas
- audit/Chronicle ideas
- approval system
- dashboard SPA
- CLI boot/setup lessons
- safe workspace execution ideas
- tenant/namespace isolation
- release/security discipline

### 13.3 What To Demote

Demote:

- fixed company-only metaphors
- mandatory Guild/Operative language
- panel-per-capability dashboard structure
- backend capabilities that have no product path
- overly complex mesh-first UX

### 13.4 What To Replace

Replace:

- "Relix is an AI company" as the main positioning
- "Prime as one-step driver button" as the core behavior
- "Rig" as product-facing adapter language
- "Brief" as the only work object name if it confuses users
- any behavior where "hey" creates a plan

---

## 14. MVP Definition

The MVP must prove one loop:

```text
Prime receives a request.
Prime creates or selects a task.
Prime starts a run or assigns an agent.
The agent/Prime uses an adapter plugin.
The adapter requests a tool call.
The kernel checks permission.
The kernel routes the tool call to a ToolSet plugin.
The tool returns a result.
The run transcript updates.
The task completes or fails.
The audit log records everything.
The dashboard shows the state.
Prime explains the result.
```

Minimum plugins:

- one adapter plugin
- one tool plugin
- one storage provider

Recommended first set:

- adapter: Claude CLI, Codex CLI, Anthropic, or OpenAI
- tool: terminal/read-only shell or simple echo/weather tool
- storage: SQLite

Do not build a full marketplace before this loop works.

Do not build many plugins before this loop works.

Do not build advanced templates before this loop works.

---

## 15. Implementation Phases

### Phase 0: Canonical Direction

Goal:

Make every AI and human understand what Relux is.

Deliverables:

- this master plan
- corrected product name in primary docs
- reconciliation between `docs/Relux spec.md` and existing Relix docs
- decision on initial vocabulary: Task vs Issue, Namespace vs Space, Agent vs
  Operative

Success:

No future agent should think Relux is only a plugin manager or only an AI
company.

### Phase 1: Core Kernel Shape

Goal:

Create the minimal Relux kernel API.

Deliverables:

- plugin manifest schema
- plugin registry/local index
- core entities: Namespace, Agent, Plugin, Task, Run, Permission, Approval,
  AuditEvent
- SQLite provider as default storage
- basic CLI commands for init/list/create

Success:

Relux can start, load local plugin manifests, store entities, and show them.

### Phase 2: Prime Core

Goal:

Make Prime the product center.

Deliverables:

- Prime chat endpoint
- intent classification
- state inspection
- Prime action router
- create task action
- start run action
- explain status/blocker action
- safe approval request path

Success:

User can chat with Prime and Prime can perform simple grounded actions without
randomly inventing plans.

### Phase 3: First Plugin-Powered Run

Goal:

Prove the core loop.

Deliverables:

- one adapter plugin
- one ToolSet plugin
- permission check for tool calls
- task leasing
- run events/transcript
- audit logging
- integration test

Success:

An agent or Prime can run a task, call a tool through the kernel, get a result,
and show it in the dashboard.

### Phase 4: Dashboard MVP

Goal:

Make the system usable.

Deliverables:

- Prime Chat
- Board
- Active Runs
- Task Detail
- Agents
- Plugins
- Permissions
- Approvals
- Audit Logs

Success:

The user can understand and operate Relux without reading terminal logs.

### Phase 5: Agent Spawning And Templates

Goal:

Let Prime create useful structures.

Deliverables:

- create agent flow
- assign task flow
- permission templates
- optional company template
- optional coding workspace template
- optional research lab template

Success:

Prime can spawn/assign agents, but company structure remains optional.

### Phase 6: Plugin Ecosystem

Goal:

Make plugins easy to build and share.

Deliverables:

- plugin SDK
- plugin templates
- plugin install/configure UX
- private/local plugin support
- plugin signing/checksums later

Success:

A developer can build a simple ToolSet or Adapter plugin without changing the
kernel.

### Phase 7: Reliability And Production

Goal:

Make Relux dependable.

Deliverables:

- plugin health checks
- retries
- run recovery
- task broker provider
- fallback adapters
- OpenTelemetry
- backup/restore
- namespace scaling
- plugin isolation

Success:

Relux can support real multi-agent workloads.

---

## 16. First Demo Target

The first demo should be small but complete.

Example:

```text
User: Prime, create a task to inspect this repo and summarize the README.

Prime:
  - classifies the message as task creation
  - creates a task
  - starts a run using a local adapter
  - calls a filesystem/read tool through a ToolSet plugin
  - summarizes the README
  - writes run events
  - writes audit events
  - marks the task completed
  - reports back in chat

Dashboard:
  - shows the task on the board
  - shows the active/completed run
  - shows transcript/tool call
  - shows audit log
```

This demo is better than a big half-working product.

---

## 17. Hard Product Requirements

These are non-negotiable.

### 17.1 Prime Must Be Smart And Grounded

Prime should feel like Codex with access to Relux actions.

Prime must understand conversational intent.

Prime must not blindly turn every message into a plan.

### 17.2 The Board Must Stay

The task/issue board is a core part of the product.

The user specifically likes the issue board concept.

### 17.3 Active Runs Must Stay

Active runs are core.

The user specifically likes seeing different runs and run states.

### 17.4 Everything Important Must Become Pluginable

Adapters, tools, storage, memory, execution, and integrations should all be
plugin-powered over time.

### 17.5 Permissions Must Be Central

Agents and Prime must not have universal access by default.

### 17.6 Dashboard Must Feel Like A Product

No placeholder HTML dashboard.

No random old control panel feeling.

### 17.7 Company Must Be Optional

Relux can be an AI company, but it is not only an AI company.

Company is a template, not the entire identity.

### 17.8 Do Not Lose The Existing Good Work

Old Relix work around runs, adapters, audit, approvals, and dashboard should be
salvaged where useful.

---

## 18. What Future AI Agents Must Not Do

Do not:

- build random features without reading this document
- treat Relux as only a plugin marketplace
- treat Relux as only an AI company
- remove the board/active runs concept
- make Prime a dumb endpoint wrapper
- let Prime create plans from greetings
- bypass permissions for convenience
- hardcode one model provider
- hardcode one database as the only future path
- start with marketplace complexity before the first run loop
- delete existing systems just because names are changing
- push key-shaped literals or secrets
- enable GitHub Actions unless explicitly needed
- leave GitHub Actions enabled after using them
- create branches unless the user explicitly asks

The user prefers direct work on `main` in this repo unless explicitly stated
otherwise.

---

## 19. How To Work In This Repository

Active workspace:

```text
D:\DATA\WORK\OpenPrem\Apps\Relix-Revised
```

Product name:

```text
Relux
```

Remote:

```text
https://github.com/itsramananshul/Relux
```

General working rules:

1. Read this document first.
2. Check git status before edits.
3. Do not overwrite unrelated user/Claude changes.
4. Work on `main` unless explicitly told otherwise.
5. Keep changes scoped.
6. Commit and push meaningful completed slices.
7. Do not enable workflows unless truly needed.
8. Disable workflows again after using them.
9. Avoid secret-shaped literals even in tests.
10. Prefer product-loop work over abstract infrastructure.

---

## 20. Immediate Next Work

The next work should not be random implementation.

The next work should be a focused reconciliation and first-loop build.

### 20.1 Documentation Reconciliation

Tasks:

- Rename product-facing docs from Relix to Relux where appropriate.
- Mark old company-only docs as legacy/inspiration unless still canonical.
- Update `docs/Relux spec.md` to say Relux, not Relix.
- Add a short `docs/README.md` or index that points to this master plan.

### 20.2 Codebase Reconciliation

Tasks:

- Identify current legacy Relix modules that map to new Relux concepts.
- Keep useful run/adapter/audit/dashboard code.
- Decide which internal names can stay temporarily.
- Avoid giant rename-only commits unless they unblock product clarity.

### 20.3 Kernel MVP

Tasks:

- define plugin manifest format
- define plugin registry/local index
- define Adapter, ToolSet, ServiceProvider, ExecutionEnvironment contracts
- define permission strings
- define audit event schema
- wire SQLite provider or adapt existing storage

### 20.4 Prime MVP

Tasks:

- Prime chat endpoint
- intent classification
- grounded state summary
- create task action
- start run action
- explain run/task status

### 20.5 First Tool Loop

Tasks:

- one local ToolSet plugin
- one adapter path
- permission check
- run event transcript
- audit log
- dashboard visibility

---

## 21. Final Product Feeling

Relux should feel like this:

```text
I open Relux.
Prime is there.
Prime knows what is happening.
I can talk naturally.
Prime can act.
Prime can create work.
Prime can spawn agents.
Agents can run tasks.
Every action goes through plugins.
Every plugin is permissioned.
Every run is visible.
Every risky action asks approval.
Every important event is audited.
The board shows the work.
Active runs show the motion.
Task detail shows the truth.
```

That is the product.

Not just an AI company.
Not just a plugin manager.
Not just a chatbot.

Relux is the Codex-like Prime control plane for agentic systems.

---

## 22. Running The Standalone MVP

The first usable Relux product boots from one command and serves its own
dashboard - no old Relix web bridge, no login, no token for the local
developer product.

To boot the Relux kernel and serve the dashboard:

```bash
cargo run -p relux-kernel -- serve
```

To run a health check on the Relux kernel:

```bash
cargo run -p relux-kernel -- health
```

That starts the local control plane and prints:

```text
Relux dashboard: http://127.0.0.1:19891/dashboard
Relux API:       http://127.0.0.1:19891/v1/relux/state

Also available:
  GET /v1/relux/health
  GET /v1/relux/tasks/:id
  POST /v1/relux/tasks/:id/execute-assigned
  POST /v1/relux/tasks/:id/reopen-and-run   # re-queue a blocked task, then run it through the unchanged run gate
  GET /v1/relux/runs/:id
  GET /v1/relux/runs/:id/events[?since=<event_id>]   # since = exclusive tail cursor; absent = full transcript
  GET /v1/relux/audit?limit=N
  GET /v1/relux/tools
  POST /v1/relux/tools/invoke
```

Open `http://127.0.0.1:19891/dashboard`. The default surface is Relux Home,
backed only by the local `/v1/relux` API:

- **Home** - The initial landing page, featuring a dynamic first-run checklist based on current system state (agents, tasks, plugins, approvals, health status) and providing direct action links to key sections. It also offers an overview of installed plugins.
- **Prime** - Chat with the local operator, including an action strip with practical example prompts to guide users in creating tasks, agents, and assigning work. It runs the same grounded `prime_turn` as the CLI, ensuring consistency between chat interactions and core system actions.
- **Work** - Standalone task board and execution history. Displays tasks with clear assignee information, enables conditional 'Run assigned' actions for delegated tasks, and supports filtering by agent and status via URL query parameters for improved navigation. Allows creating tasks and starting runs directly from the board.
- **Crew** - Create and manage local agents. Each agent's card now includes direct links to their assigned queued and running tasks on the Work page, facilitating workload overview and navigation.
- **Plugins** - install/remove plugins through the durable lifecycle
  (`/v1/relux/plugins/*`).
- **Approvals** - manage pending approvals and agent permissions (`/v1/relux/approvals`, `/v1/relux/permissions`).
- **Health** - local readiness/diagnostics surface: state counts, plugin/tool/
  adapter status, Prime autonomy status, AI mode, and the package/check command
  hints (`/v1/relux/health`, `/v1/relux/tools`, `/v1/relux/adapters`,
  `/v1/relux/prime/autonomy`). It depends only on the local control plane, never
  the old bridge.

These seven surfaces are the entire standalone navigation. The old bridge-backed
Relix pages are not part of this shell and do not appear in its nav; they remain
reachable only at their legacy paths (labelled legacy when visited directly).

The dashboard bundle is the committed Vite build at
`crates/relix-web-bridge/dashboard-dist`; `relux-kernel` serves it directly
(SPA history fallback included). Rebuild it with `npm run build` in
`apps/dashboard` after changing the frontend. If the bundle is missing,
`/dashboard` returns an honest "not built" notice (HTTP 503), never a panic.

Configuration:

- `RELUX_DB` - the durable SQLite store (default `dev-data/relux/local.db`).
- `RELUX_HTTP_ADDR` - the bind address (default `127.0.0.1:19891`). When the
  port is already in use (commonly because Relux is already running), `serve`
  stops with an actionable message that names the busy address, points at
  `http://127.0.0.1:19891/dashboard` to check for a running instance, and gives
  the exact command to pick another port (`RELUX_HTTP_ADDR` for a source
  checkout, `Start-Relux.ps1 -Port <port>` for the bundle) - never a bare OS
  error and never an auto-picked port. The bundle launcher's preflight repeats
  this same guidance. Because the two surfaces are written in Rust and
  PowerShell respectively, `scripts\check-port-guidance.ps1` (run by the release
  gate) reads both and asserts they stay in lockstep - both name the conflict
  "already in use", point at `/dashboard`, and show `Start-Relux.ps1 -Port`,
  and neither promises to auto-pick a port - so the wording cannot drift.
- `RELUX_DASHBOARD_DIST` - override the dashboard bundle directory.

Local first-release checks:

```powershell
# Quick gate.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-first-release-check.ps1
# Quick gate + the full standalone end-to-end smoke (run before cutting a release).
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-first-release-check.ps1 -FullE2E
# The end-to-end smoke on its own.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-e2e-smoke.ps1
# Package a portable local bundle.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1
```

The check script builds the dashboard, tests and lints the Relux kernel/core,
builds the release binary, runs `doctor`, runs the static contract checks
(`check-port-guidance.ps1`, `check-smoke-adapter-boundary.ps1`), and smoke-tests
Prime against a temporary `RELUX_DB`. The Prime smoke proves the **honest local
boundary** (§8.1): Prime creates a free-form "inspect this repo" task, and running
it on the deterministic local adapter **fails closed** — a non-zero exit with
actionable guidance and the task parked `Blocked` — never a silent hang and never
a fabricated "done". (Local Prime does no external work; real agent work is a
configured Claude/Codex adapter, exercised by the E2E smoke's opt-in real-adapter
steps.) `-SkipSmoke` skips that quick Prime smoke; `-FullE2E` additionally runs
`scripts\relux-e2e-smoke.ps1` (it reuses the just-built release binary).
`scripts\check-smoke-adapter-boundary.ps1` is a static drift guard that pins this
local-vs-real-adapter honesty contract so the smokes cannot quietly revert to
making local Prime fake external work to turn the gate green.

`scripts\relux-e2e-smoke.ps1` is the standalone first-release end-to-end smoke.
It proves the first version of the product is actually usable - not just
unit-tested - by driving the release binary through every critical local flow
against a **throwaway temporary `RELUX_DB`** (it never touches the real
`dev-data\relux\local.db` or any real `serve` instance). It records PASS/FAIL/SKIP
and proves: `doctor` health plus full bundled plugin/adapter coverage (echo,
status, local-prime, claude-cli, codex-cli); Prime chat (a greeting calls no
tool, "what tools can you use?" lists the real tools, a status request invokes
the status tool, an echo request invokes the echo tool); the tool CLI (`tools`
listing + `tool invoke ... echo.say {json}` round trip); Plugin Runtime v1 (an
in-script loopback HTTP server is installed/configured/invoked through the
kernel and its output flows back); adapter runtime controls (enable with a fake
command + disable, **never spawning real Claude/Codex**); the autonomy loop's
**honest fail-closed** (a free-form "inspect this repo" task created through Prime
is driven by one tick to a terminal Failed run + parked `Blocked` with guidance —
never a hang, never a fabricated Completed); the **positive local path** over HTTP
(a task carrying an echo.say `tool_call` directive is created, executed as
assigned, and both the run and the task honestly reach `completed` — local Prime
runs what it CAN fulfil, for real); the optional real Claude/Codex adapter run +
Prime-brain chat (opt-in, never bypass flags); and the `serve` HTTP endpoints
(`/dashboard`, `/v1/relux/state`, `/v1/relux/prime/autonomy`, `/v1/relux/tools`).
Flags: `-SkipBuild`, `-SkipServe`, `-SkipLoopback`, `-KeepTemp`,
`-RunRealClaudeAdapter`, `-RunRealCodexAdapter`.
It always cleans up its temp DB, server, jobs, and processes, and exits non-zero
on any failure.

The package script creates `dist\relux-local-<version>-windows-x64\` with the
release binary, dashboard dist, bundled example plugins, docs, and
`Start-Relux.ps1`. These are local release helpers only; GitHub Actions remain
disabled unless explicitly enabled by the user.

### Release Candidate Packaging (local-first)

`scripts\relux-package-local.ps1` produces the first shareable Relux release
candidate as a self-contained Windows bundle. It is deliberately local-first: a
portable folder + zip you hand to someone, not an installer, signed artifact, or
hosted download.

Commands:

```powershell
# Quick package: quick readiness gate, then package.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1
# Full verified package: quick gate + standalone end-to-end smoke, then package.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1 -FullE2E
# Fast repackage, no gate (still builds the release binary if missing).
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\relux-package-local.ps1 -SkipChecks
```

What the bundle includes (`dist\relux-local-<version>-windows-x64\`, plus a zip):

- `relux-kernel.exe` - the control-plane binary.
- `dashboard-dist\` - the built dashboard served at `/dashboard`.
- `examples\relux-plugins\` - bundled example plugins/adapters.
- `docs\RELUX_MASTER_PLAN.md` + `README.md` - the design plan and reference.
- `Start-Relux.ps1` - a robust launcher: it sets `RELUX_HTTP_ADDR`, `RELUX_DB`
  (under `.\data\local.db` in the bundle), and `RELUX_DASHBOARD_DIST`, prints the
  dashboard URL, supports `-Port`, and fails clearly if `relux-kernel.exe` is
  missing. Before launching it preflights `127.0.0.1:<port>`: if the port is
  already in use it stops with an actionable message (open the running instance,
  or re-run with `-Port <free port>`) rather than printing a dashboard URL that
  points at the other process.
- `VERSION.txt` (machine-friendly) + `RELEASE-NOTES.txt` (human-friendly) -
  release metadata: version, git commit (short + full), git branch, working-tree
  cleanliness, build timestamp (UTC), the verification mode that produced the
  artifact (`full-e2e` / `quick` / `skipped`), and the supported core loops:
  Prime chat, Work/task run, plugins, loopback tool runtime, adapter runtime
  controls, and autonomy.

Hygiene: `dist\` is gitignored and never committed. The package step itself never
spawns a server or writes a temp DB; the readiness gate it invokes runs all
smokes against a throwaway `RELUX_DB` and always cleans up its temp DB, server,
jobs, and processes.

What remains intentionally local-first (out of scope for this RC):

- No installer, code signing, auto-update, or hosted/download distribution - you
  share the folder/zip directly.
- Windows x64 only (the script builds and labels a `windows-x64` bundle). No
  cross-OS bundles are produced here.
- The standalone API binds loopback and is now gated by a single-admin local
  operator login (post-v0.1.4); it is still not a multi-user or production
  surface (one admin, http loopback with no transport TLS). Sessions are now
  persisted locally (a hash of the sid + deadlines, gitignored) so they survive a
  `serve` restart, but the surface is still single-operator local-first.
- GitHub Actions stay disabled; releases are cut by hand with this script.

### Release history (local Windows bundles)

Relux ships as hand-cut, local-first Windows bundles (no installer, no hosted
download). The version is the `relux-kernel` / `relux-core` crate version and is
stamped into `relux-kernel doctor`, `/v1/relux/health`, and the bundle's
`VERSION.txt`. Build a bundle with `scripts\relux-package-local.ps1 -FullE2E`.

- **v0.1.38** (2026-06-13) — **Prime plugin-use + Crew usability** rollup. The `relux-kernel` /
  `relux-core` crates move `0.1.37` → `0.1.38` in lockstep, packaging two post-v0.1.37 slices into a
  fresh Windows bundle on top of all v0.1.37 work (RELUX_MASTER_PLAN §8.1 / §10.1 / §11.6;
  `docs/prime-tool-use.md`; built reference-first per `docs/reference-driven-development.md`).
  Headline: **the install→configure→use path closes and Crew is never blank.**
  - **Plugin post-configure "Prime can use this now" cue + Prime-catalog refresh** (§11.6 / §10.1).
    After a plugin is installed and configured (MCP server registered or command tool activated), the
    flow surfaces the concrete `primeUseCue` and refreshes the Prime tool catalog so the freshly
    configured tool is immediately visible to Prime (`docs/prime-tool-use.md` verified install→use
    steps 4–5).
  - **Prime chat install/configure result-card `next_step` aligned to the concrete `run the <tool>
    tool` phrase** (§11.6). The post-configure result card's `next_step` now matches the exact
    `primeUseCue` phrasing the user can act on (`docs/prime-tool-use.md` Result step).
  - **Crew page is never blank/useless** (§8.1 "Crew → Prime Brain"). The dashboard Crew view
    surfaces the Prime Brain and renders actionable empty / populated / error states instead of an
    empty page.
  - Dashboard typecheck / tests / build green. The full-e2e release gate
    (`scripts\relux-package-local.ps1 -FullE2E`) is run at package time. All reads/writes hit real
    kernel state; no new authority is added. Every safety property from v0.1.37 holds.
- **v0.1.37** (2026-06-13) — **Stuck/no-activity run fix** rollup. The `relux-kernel` / `relux-core`
  crates move `0.1.36` → `0.1.37` in lockstep, packaging the post-v0.1.36 fix into a fresh Windows
  bundle on top of all v0.1.36 work (RELUX_MASTER_PLAN §8.1; built reference-first per
  `docs/reference-driven-development.md`). Headline: **local-prime fails honestly on external-work
  tasks, and a started run always reaches a terminal state** (§8.1). A task whose human title is
  obvious external work (clone a repo, import/install a plugin, download from a URL) — the dashboard
  New Task form shape, with no `prime_request` key — was previously either echo-faked as "done" or,
  when started via the bare `/start` route, left dangling in `Running` with only a `run_started`
  event ("No activity forever"). `relux-core` adds `title_requires_external_execution` (a narrow
  keyword safety-rail) and `local_prime_cannot_fulfill(title, input)`, which combines it with the
  existing `is_unfulfillable_local_request` and short-circuits to fulfillable for a `tool_call` /
  `tool_plan` directive. `effective_run_adapter` + `execute_local_run` key the redirect-to-brain /
  fail-closed decision on `local_prime_cannot_fulfill`, so an external-work title routes to a
  configured real brain or fails closed (`Failed`/`adapter_missing` + `Blocked`) with actionable
  guidance — never echo-fakes, never hangs. `POST /tasks/:id/start` routes through
  `execute_assigned_run` (not bare `start_run`), so a started run always reaches a terminal state; a
  fail-closed start returns `200` with the terminal run/task + a refused message. The dashboard's
  **Run (Assigned)** no longer double-starts, an assigned created task is runnable, and `startTask`
  carries the optional refused reason. Core / kernel / server regression tests pin the new semantics.
  All reads/writes hit real kernel state; no new authority is added. Per-slice `cargo test` +
  `clippy --all-targets -D warnings` clean on `relux-core` / `relux-kernel`; dashboard typecheck /
  tests / build green. The full-e2e release gate (`scripts\relux-package-local.ps1 -FullE2E`) is run
  at package time. Every safety property from v0.1.36 holds.
- **v0.1.36** (2026-06-13) — **Orchestration result card becomes a run control + plugin-use proof**
  rollup. The `relux-kernel` / `relux-core` crates move `0.1.35` → `0.1.36` in lockstep, packaging
  two post-v0.1.35 Prime-usability slices into a fresh Windows bundle (`docs/prime-tool-use.md`;
  RELUX_MASTER_PLAN §10.4 / §11.1 / §17.1; `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` §6; built
  reference-first per `docs/reference-driven-development.md`). Headlines: (1) **The orchestration
  result card becomes a real run control** (§10.4 / §11.1 / §17.1; UI only, no kernel change) — the
  v0.1.35 card rendered the briefs but pushed the operator to a conversational chip or the Advanced
  panel to actually run. The card now owns the run: an explicit **Run orchestration** button starts
  the EXISTING non-blocking `run-async` job (the same route the standalone `OrchestrationPanel`
  uses), a 1s poll renders the live phase / round / per-brief progress and the terminal
  completed/failed/canceled state, and a restart-honest **interrupted** callout appears when no
  live worker is driving the run (reusing the shared `orchestration.ts` job helpers). It reconnects
  to an already-active job instead of double-running (read-only mount reconnect + 409 → reconnect),
  surfaces an over-cap refusal (429) verbatim, deep-links each brief's run into a populated
  `/work?run=<id>`, and refreshes the durable record once a run finishes so outcomes are real — all
  without auto-running on render (only an operator click starts a job; each brief still gates at run
  time). The redundant "Run this orchestration" suggestion chip is filtered from the generic row
  (`isRunOrchestrationSuggestion`) so there is one clear primary run path; the chip still works if
  typed. Focused render tests pin the button, the deep links, the no-fabricated-link rule, the chip
  filtering, and a no-stale-`dashboard-dist` bundle check; rebuilt bundle committed. (2) **Plugin
  usability proof — a configured non-echo command tool is invokable from chat through the governed
  gate** — the chat-path invocation of a configured (non-echo) command tool is now pinned through the
  governed tool-approval gate by a test, closing the lone remaining gap in the install → configure →
  Prime-use path so it is verified end-to-end rather than asserted by vibes (`docs/prime-tool-use.md`,
  `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` §6). All reads/writes hit real kernel state; no new authority
  is added, nothing auto-runs without an explicit gate. Per-slice `cargo test` +
  `clippy --all-targets -D warnings` clean on `relux-core` / `relux-kernel`; dashboard typecheck /
  tests / build green. The full-e2e release gate (`scripts\relux-package-local.ps1 -FullE2E`) is run
  at package time. Every safety property from v0.1.35 holds.
- **v0.1.35** (2026-06-13) — **Grounded orchestration result card** rollup. The `relux-kernel` /
  `relux-core` crates move `0.1.34` → `0.1.35` in lockstep, packaging the post-v0.1.34 fix into a
  fresh Windows bundle (`docs/prime-tool-use.md`; RELUX_MASTER_PLAN §10.4 / §11.1 / §17.1; built
  reference-first per `docs/reference-driven-development.md`). The first real coordination loop in
  Prime chat becomes legible. Headlines: (1) **Explicit multi-agent goal becomes a grounded chat
  result card** — an explicit goal (`orchestrate research the options, build a prototype, and write
  the docs`) already created a durable `Orchestration`, but the chat turn returned only prose. The
  executed `OrchestrateGoal` turn now carries the record as structured data: `PrimeTurn` gains an
  optional `orchestration` field (skip-if-none, wire-compatible) set to the durable record, so the
  fan-out is legible the moment Prime mints it. Casual ideation stays conversation and mints no
  record (§10.4 / §11.1). (2) **One-click governed next actions, never a privileged path** — the
  turn attaches `suggested_actions`: "Run this orchestration" (the explicit, governed batch) and a
  pre-filled (not auto-sent) "Hire a <role> agent" for each role that fell back to Prime. Each is an
  ordinary user message; creating an orchestration still runs nothing on its own (§11.1 / §17.1).
  (3) **Skill-aware role grounding** — planner grounding resolves role→agent by the agent's id
  keyword or its declared specialty skill, so a manually-configured operative with an opaque id but a
  `research` skill matches the same as a conversational `researcher` hire. Width still comes from the
  configurable `PrimeAgentPolicy` (no hidden cap) (§10.4). (4) **Dashboard renders the result card
  (UI only, no kernel change)** — `OrchestrationResultCard` shows the ordered briefs (assignee + role
  + outcome), the specialist/Prime-fallback split, the planner's honest notes, and a Work link; pure
  `orchestrationAssignmentSummary` / `stepIsPrimeFallback` helpers + unit tests cover the split;
  rebuilt `dashboard-dist` committed (§17.1). All reads/writes hit real kernel state; no new authority
  is added, nothing auto-runs without an explicit gate. Per-slice `cargo test` +
  `clippy --all-targets -D warnings` clean on `relux-core` / `relux-kernel`; dashboard typecheck /
  tests / build green. The full-e2e release gate (`scripts\relux-package-local.ps1 -FullE2E`) is run
  at package time. Every safety property from v0.1.34 holds.

- **v0.1.34** (2026-06-13) — **Conversational agent creation** rollup. The `relux-kernel` /
  `relux-core` crates move `0.1.33` → `0.1.34` in lockstep, packaging the post-v0.1.33 fixes into a
  fresh Windows bundle (`docs/prime-tool-use.md`; RELUX_MASTER_PLAN §6 / §7.1 / §7.3 / §7.5 / §8.1;
  built reference-first per `docs/reference-driven-development.md`). Headlines: (1) **Hire an
  operative from Prime chat, honestly** — Prime's chat now classifies natural hire phrasing
  (`make a coding agent for this repo`, `<verb> a <role> agent`) as an `AgentCreation` turn via
  `creates_an_operative()`, fail-closed so a task that merely references an agent stays a
  `TaskCreation`. The new pure `prime_agent_create` module resolves an adapter/brain preference
  (`uses Claude` / `run codex on`) only when that adapter plugin is installed — checked against the
  new `StateSummary.available_adapter_ids` — and otherwise falls back honestly to `local-prime`;
  it never invents or enables an adapter (ref: openclaw `common.ts::normalizeToolModelOverride`)
  (§6 / §8.1). (2) **Requested-capability honesty + approval-gated grant follow-ups** —
  `requested_capabilities()` detects asks like "can read GitHub" / terminal access; the operative is
  created with no permissions, the reply says setup is needed, and `attach_suggestions` offers a
  one-click `send:false` "Grant X access to <agent>" that routes through the unchanged
  approval-gated `PermissionChange` path (now roster-aware via `resolve_assignee` so the follow-up
  lands on the real agent id). A deterministic duplicate-name guard returns an honest reply rather
  than a kernel `AgentExists` error; no auto-grant, no new authority, no danger flags (§7.1 / §7.3).
  (3) **Dashboard surfaces the conversational hire (UI only, no kernel change)** — Prime chat renders
  an `AgentCreatedCard` (built from the pure `agentCreatedView`) showing the operative's name/id, the
  adapter it runs on (human brand + raw id, brain-validated slot preferred), brain-shaped
  role/persona, a "View in Crew" link, and a "Give it work" pre-fill; a requested sensitive
  capability shows as NEEDING SETUP with a "Grant X access to <agent>" button that pre-fills the
  existing approval-gated follow-up (nothing granted until approved). Crew exports a `CrewMemberCard`
  with honest placeholders for missing fields; rebuilt `dashboard-dist` committed (§7.3 / §7.5). All
  reads/writes hit real kernel state; no new authority is added, nothing auto-runs without an explicit
  gate. Per-slice `cargo test` + `clippy --all-targets -D warnings` clean on `relux-core` /
  `relux-kernel`; dashboard typecheck / tests (687) / build green. The full-e2e release gate
  (`scripts\relux-package-local.ps1 -FullE2E`) is run at package time. Every safety property from
  v0.1.33 holds.

- **v0.1.33** (2026-06-13) — **Brain-aware run routing + plugin activation honesty** rollup. The
  `relux-kernel` / `relux-core` crates move `0.1.32` → `0.1.33` in lockstep, packaging the
  post-v0.1.32 fixes into a fresh Windows bundle (`docs/prime-tool-use.md`, `docs/mcp.md`;
  RELUX_MASTER_PLAN §8.1 / §8.2 / §17.5 / §10.2 / §10.3; built reference-first per
  `docs/reference-driven-development.md`). Headlines: (1) **Brain-aware run routing — Prime work
  uses the real adapter or fails closed** — a free-form Prime goal assigned to the local-prime
  default now routes to the real adapter the operator's configured brain resolves to (Claude / Codex
  CLI) instead of silently echoing on local-prime, and fails closed with a "Set Prime's brain" setup
  action when no real brain is configured. Confined to the operator-initiated `execute_assigned_run`
  chokepoint via `effective_run_adapter`; `start_run` is unchanged, so the §17 autonomy tick keeps
  the deterministic local path and never auto-spawns a paid CLI. The brain choice is a secret-free
  `PrimeBrainPreference` snapshot re-synced from the on-disk `AiConfig` before every run, and the run
  record + transcript are stamped (`adapter_selected`) with the adapter actually used (§8.1). (2)
  **Governed command-tool bridge for source-only plugins** — a source-only import (no
  `relux-plugin.json`, no detected candidate) is usable without hand-editing JSON via
  `POST /v1/relux/prime/actions/configure-command-tool`, which re-validates the operator's argv
  recipe through the unchanged `parse_command_tool_input` + `configure_command_tool` (argv-only, no
  shell, no danger flag, confined cwd, approval always Required) and stages a High-risk, confirm-gated
  Prime proposal; no command is inferred from repo content (§8.2 / §10.2 / §10.3). (3) **Guided
  secret/env setup for managed-stdio MCP activation** — `GET`/`POST /v1/relux/mcp/servers/:id/env-setup`
  stores an inline value write-only or references an existing secret and maps `ENV_VAR → {secret}`
  onto an existing managed-stdio server, re-validating names before any mutation and optionally
  re-discovering; the `McpEnvSetupForm` wires it into the Prime chat activation result and the
  Plugins card, values are never shown after save, and the path runs no source code and never returns
  a value (§17.5 / §8.2). All reads/writes hit real kernel state; no new authority is added, imports
  run no repo code, and nothing auto-runs without an explicit gate. Per-slice `cargo test` +
  `clippy --all-targets -D warnings` clean on `relux-core` / `relux-kernel`; dashboard typecheck /
  tests / build green. The full-e2e release gate (`scripts\relux-package-local.ps1 -FullE2E`) is run
  at package time. Every safety property from v0.1.32 holds.

- **v0.1.32** (2026-06-13) — **Prime conversation-first + plugin-tool honesty** rollup. The
  `relux-kernel` / `relux-core` crates move `0.1.31` → `0.1.32` in lockstep, packaging the
  post-v0.1.31 fixes into a fresh Windows bundle (`docs/prime-tool-use.md`; RELUX_MASTER_PLAN §8 /
  §8.2 / §10.1 / §10.2 / §10.3; built reference-first per `docs/reference-driven-development.md`).
  Headlines: (1) **Dev/test echo fixture hidden from Prime's brain by default** — the built-in
  echo/dev fixture is omitted from both the tool catalog and the brain decision prompt unless the
  new `RELUX_DEV_FIXTURES` master switch is set, so Prime never proposes a test fixture as if it
  were a real installed capability (`builtin.rs` / `lib.rs` / `server.rs` / `state.rs`). (2)
  **Install → configure → Prime-can-use is regression-proofed end-to-end** — a standalone smoke
  (`scripts/smoke-plugin-install-to-prime-use.ps1`) plus new `server.rs` route coverage drive the
  full real-route path (import a plugin → activate the detected candidate → confirm Prime can use
  the resulting governed tool) so the install-to-usable journey cannot silently regress; documented
  in `docs/prime-tool-use.md` (§8 / §8.2 / §10.2 / §10.3). (3) **Prime defaults to conversation,
  with explicit / staged action** — the brain decision prompt is tiered so a plain message is
  answered as conversation by default and action is taken only when explicitly intended (and stays
  staged / confirm-gated), contextless cold-start work chips are dropped, and the
  conversation→action contract is pinned by tests (`apps/dashboard/src/prime.ts` +
  `prime.test.ts`); the rebuilt tracked `dashboard-dist` is committed (§10.1). All reads/writes hit
  real kernel state; no new authority is added, imports run no repo code, and nothing auto-runs
  without an explicit gate. Per-slice `cargo test` + `clippy --all-targets -D warnings` clean on
  `relux-core` / `relux-kernel`; dashboard typecheck / tests / build green. The full-e2e release
  gate (`scripts\relux-package-local.ps1 -FullE2E`) is run at package time. Every safety property
  from v0.1.31 holds.

- **v0.1.31** (2026-06-13) — **Prime brain setup + plugin install-to-usable** rollup. The
  `relux-kernel` / `relux-core` crates move `0.1.30` → `0.1.31` in lockstep, packaging the whole
  post-v0.1.30 line into a fresh Windows bundle (`docs/prime-tool-use.md`; RELUX_MASTER_PLAN §8 /
  §8.2 / §10.1 / §10.2 / §10.3 / §14 / §22; built reference-first per
  `docs/reference-driven-development.md`). Headlines: (1) **Product-grade Prime brain setup + a safe
  quick probe** — the Health → Prime Brain panel makes powering Prime obvious (recommended
  Claude / Codex / OpenRouter ordered first, **Local** tagged *fallback / test*, a "Set up a real
  brain →" banner whenever Prime is on the Local fallback), and `POST /v1/relux/ai/probe { brain? }`
  answers "is this brain usable right now?" with a clear status + secret-free `detail` + next step: a
  CLI brain runs `<bin> --version` ONLY when enabled and on `PATH` (reusing the assigned-run spawn
  contract `probe_cli_version` → `run_adapter_command`: argv-only, empty stdin, short timeout, output
  cap, secret redaction, **no `--dangerously-skip-permissions`/bypass flag**), OpenRouter reports
  whether its key resolves **without** a billable request, Local is always ready; no raw key is ever
  stored or shown (§14 / §10.1). (2) **Live chat brain probe** — `POST /v1/relux/ai/probe/live`
  proves Prime can complete a real turn (not just that the binary runs): ONE tiny bounded prompt
  through the selected / resolved brain, classified `ready` / `not_configured` / `missing_key` /
  `auth_failed` / `timeout` / `failed` / `unsupported` with a redacted, truncated `sample` and a
  `duration_ms`; CLI brains use the **same safe adapter invocation a real turn uses**
  (`build_adapter_args` → `run_adapter_command`, no bypass flag, 60 s timeout, 16 KiB cap,
  `parse_adapter_result` + auth/sign-in detection), OpenRouter sends one small (billable) request and
  returns WITHOUT one when no key resolves, Local contacts no provider — mirroring Hermes'
  prove-with-a-real-completion-then-classify shape
  (`reference/hermes-agent-main/agent/auxiliary_client.py`). It is a **setup diagnostic only**:
  the **Test live chat** button runs only on a deliberate click, warns it may use the real
  provider / CLI and incur usage, and creates no task / run and grants no broader permission. (3)
  **First-run guided launchpad** — a dashboard-side slice that turns Home's first-run checklist (§22)
  into a coherent path to a useful Prime (try the first useful turn, resume paused work, inspect
  stuck work), derived purely from real `state` + `/v1/relux/oversight` reads through the existing
  `ReadinessGuide`; the fallback Local brain stays labelled honestly and the new attention stages are
  *warn*, never blockers, so they never fake "setup needed" on a working instance. (4)
  **Prime-staged GitHub plugin import from chat** — "install owner/repo as a plugin" /
  "import https://github.com/owner/repo as plugin" is recognized
  (`PrimeAction::InstallPluginFromGithub`), the repo is parsed into a canonical, **credential-free**
  https URL (anything before `github.com/`, including `user:token@`, is dropped; non-GitHub host → no
  match), and the **safe manifestless import** is staged behind a `RiskLevel::High` human
  confirmation, then shows the installed plugin + detected capability candidates (§8 / §10.2 / §10.3).
  (5) **Backend-governed plugin-install action — one auditable chokepoint** —
  `POST /v1/relux/prime/actions/install-plugin` re-validates server-side
  (`canonicalize_github_repo_url` rebuilds or rejects a swapped host / embedded credential / extra
  path; an echoed `plugin_id` that does not match the id re-derived from the repo is a 400), runs the
  EXISTING manifestless installer + the SAME read-only candidate scan internally (no duplicated shell
  code), and returns one structured envelope (plugin record, canonical source, candidates +
  `candidate_count`, honest `next_actions`, the `no_code_executed` guarantee, the closed
  `approval_id`) so a headless / API Prime has a single execution path. (6) **Prime-guided activation
  of detected candidates** — "configure the first candidate" / "enable the MCP server from <plugin>"
  / "turn that script into a tool" stages a confirm-gated `POST /v1/relux/prime/actions/configure-candidate`
  (`PrimeAction::ConfigurePluginCandidate`) that re-reads the plugin's candidates from a fresh
  read-only scan, re-resolves the selection without trusting a client command (exact id /
  unique-with-candidates / fuzzy name, honest 400 on ambiguity), and activates through the
  **unchanged** MCP registry (`register_mcp_server` / `register_mcp_stdio_server`) or command-tool
  (`parse_command_tool_input` + `configure_command_tool`) paths — metadata / recipe only, no source
  code runs, and the resulting tool stays gated until invoked (§8.2 / §10.2 / §10.3). (7) **Guided
  post-activation MCP discovery** — after Prime registers an MCP candidate, the configure-candidate
  route runs ONE bounded `tools/list` probe against the freshly-registered server **off the kernel
  lock** (`discover_and_classify_mcp_tools` via `spawn_blocking`) so the operator immediately sees
  what Prime can now use; it is best-effort (a probe failure becomes actionable guidance — "map
  secrets, then Discover" / "Start it on the MCP page" — never a failed activation), LISTS tools only
  (never calls one), and never downgrades a tool's fail-closed classification (unclassified ⇒
  `needs_approval`). Reference-driven (Hermes `hermes_cli/plugins_cmd.py` /
  `hermes_cli/mcp_config.py` — resolve / clone / configure-is-not-run / discovery-first; openclaw
  `approval-classifier.ts` — the stateful path is always confirmation-gated). All reads/writes hit
  real kernel state; no new authority is added, imports run no repo code, and nothing auto-runs
  without an explicit gate. Per-slice `cargo test` + `clippy --all-targets -D warnings` clean on
  `relux-core` / `relux-kernel`; dashboard typecheck / tests / build green; tracked `dashboard-dist`
  in sync. The full-e2e release gate (`scripts\relux-package-local.ps1 -FullE2E`) is run at package
  time. Every safety property from v0.1.30 holds.

- **v0.1.30** (2026-06-13) — **agentic tool-use + MCP-surface completion** rollup. The
  `relux-kernel` / `relux-core` crates move `0.1.29` → `0.1.30` in lockstep, packaging the whole
  post-v0.1.29 line into a fresh Windows bundle (`docs/prime-tool-use.md`, `docs/mcp.md`;
  RELUX_MASTER_PLAN §8.2 / §9.6.1 / §10.1 / §10.5 / §14 / §17.1; built reference-first per
  `docs/reference-driven-development.md`). Headlines: (1) **Auto-detected CLI brain** — Prime
  auto-adopts an enabled, on-`PATH` Claude / Codex CLI as its brain when none is selected
  (`resolve_brain` / `available_cli_brains` in `ai.rs`, threaded at `run_prime` / ai-status /
  health / doctor), so it is a real chat agent out of the box instead of silently falling back to
  Local. (2) **Run watchdog** — a server-side watchdog recovers stale `Running` runs as
  `RunFailureClass::Stale` so no run hangs silently (§9.6.1). (3) **Structured capability
  candidates** — a manifestless / imported-repo scan yields structured per-capability candidates
  (`capability_detect.rs`) so install-to-usable is a real per-candidate path (one-click `mcp_register`
  vs. manual `pending`), not a dead end (§8.2). (4) **Governed command tools** — a detected
  `cli_command` candidate (npm `bin`, Python `[project.scripts]`, Cargo `[[bin]]`) becomes a real
  approval-gated tool: the operator confirms a validated argv recipe (`relux_core::command_tool`,
  argv-only / no shell / no danger flag / bounded / confined cwd) stored as `CommandToolConfig`, and
  it runs ONLY through the existing gated invoke path with timeout + kill-on-timeout + bounded,
  secret-redacted output (`command_exec.rs`); nothing runs on import or at configure time (§8.2).
  (5) **Prime tool / plugin awareness** — the Prime Agent Loop entry is brain-driven (the UNCHANGED
  fail-closed `reconcile_intent`, keyword classifier demoted to fallback rail), the decision prompt
  is handed the runnable tool inventory (`render_tool_inventory`), and a new `GET /v1/relux/prime/tools`
  + a "Tools Prime can use" panel surface the catalog, so an installed / configured tool is finally
  usable from natural-language chat — every execution still flowing through the single
  `prime_invoke_tool` → `invoke_tool` chokepoint (§10.1 / §10.5 / §17.1). (6) **Continuous tool use**
  — live MCP tool names land in Prime's *first* decision (`decision_time_mcp_catalog`, bounded /
  off-lock / TTL-cached / fail-closed) and, after **Approve & run** / **Allow always**, the dashboard
  auto-resumes the paused loop (`POST /v1/relux/prime/agent/continue`) into approve → run → continue
  without a second typed prompt. The MCP surface also completed over managed stdio: **resources v1**
  (`resources/list` + `resources/read`), **prompts v1** (`prompts/list` + `prompts/get`), and a gated,
  default-deny **sampling v1** (`sampling/createMessage`, advertised only when serviceable) — leaving
  only resource **subscriptions** out of scope. Plus **local-prime fails closed** on free-form
  external goals (no dangling "running but nothing happens" runs) and **manifestless plugin import
  made unmistakable** (no `relux-plugin.json` required, actionable after install). All reads/writes
  hit real kernel state; no new authority is added and nothing auto-runs without an explicit gate.
  Full-e2e release gate **PASS** (`scripts\relux-package-local.ps1 -FullE2E`); `cargo test` +
  `clippy --all-targets -D warnings` clean on `relux-core` / `relux-kernel`; dashboard
  typecheck / tests / build green; tracked `dashboard-dist` in sync. Every safety property from
  v0.1.29 holds.
- **v0.1.29** (2026-06-12) — **recovery + Cross-Guild Inbox** rollup. The `relux-kernel` / `relux-core`
  crates move `0.1.28` → `0.1.29` in lockstep, packaging the whole post-v0.1.28 line per
  `docs/relix-dashboard-design.md` §6.6–§6.11 and `relix-execution-and-issue-design.md` §3.3b. (1) **Safe
  task reparenting v1** (`POST /v1/relux/tasks/:id/parent`, the create-path exist / namespace / cycle
  validation reused, structural only, compact Move-under… / Remove-parent control whose candidates exclude
  self + descendants client-side). (2) **Drag-to-column status movement v1** — drag a Work card onto a
  column to Block / Cancel, validated and additive. (3) **Keyboard-accessible board movement v1** — a
  descriptive `aria-label` + visible helper on the status-move select, an honest reason for finished tasks.
  (4) **Reopen-blocked-work v1** — re-queue a blocked task as a run-lifecycle action (`POST
  /v1/relux/tasks/:id/reopen`), not a status decree, validated + eligibility-gated; plus **one-click Reopen
  & run** that chains the re-queue into the assigned-run path through both existing gates. (5) **Recovery
  decision cards v1** for failed / blocked work — a deterministic model (root cause + recommendation +
  existing-route choices) with no new authority. (6) **Investigate with Prime v1** — seed the chat from a
  recovery card with a safe, redacted diagnosis. (7) **Diagnostic narrative pass v1** — an explicit,
  operator-triggered, **read-only** `POST /v1/relux/runs/:id/diagnose` (`run_diagnosis.rs` pure model) that
  hands a bounded + redacted run context to the configured brain off-lock for a concise four-part narrative
  (likely cause / evidence / next action / uncertainty), mutating nothing, gated to failed runs, with a
  clean no-provider fallback and an inline result on the card. (8) **Cross-Guild Inbox v1** — a new
  read-only `GET /v1/relux/inbox` projection (`server.rs::get_inbox`, sibling of `get_oversight`) composes
  the whole Guild's attention items — pending approvals, hard-failed runs (**excluding** the ones silently
  auto-retrying, per `relix-execution-and-issue-design.md` §3.3b), blocked tasks, and a paused Prime
  continuation — into unified `InboxItem`s (stable id / kind / severity / title / summary / related ids /
  recommended action kinds / link); a top-level **Inbox** page + sidebar badge
  (`apps/dashboard/src/pages/Inbox.tsx`, pure `inbox.ts`) renders the prioritized queue, every action
  reuses an existing route (open-approval / retry / reopen[-and-run] / diagnose / investigate / continue /
  inspect) — **no new authority, no auto-run**. It also gained **inline approval decisions** in place,
  honest logical-clock **ageing / SLA** buckets (fresh / waiting / stale / overdue; kernel has no
  wall-clock, so age is kernel-event ticks), a URL-shareable **cross-Guild search / filter**, and
  **cross-item subtree grouping** (collapse a stalled subtree into one card on the real `parent_task`
  edge). All reads/writes hit real kernel state; honest empty states + bounded `truncated` flags. No
  master-plan safety property is weakened. `cargo test` + `clippy` clean on `relux-core`/`relux-kernel`;
  dashboard `tsc`/`npm test` green; `dashboard-dist` in sync. Every safety property from v0.1.28 holds.
- **v0.1.28** (2026-06-12) — **dashboard Work-board oversight** rollup. The `relux-kernel` / `relux-core`
  crates move `0.1.27` → `0.1.28` in lockstep, packaging the post-v0.1.27 oversight line per
  `docs/relix-dashboard-design.md` §6.x: (1) **Board Oversight v1** — a composed `GET /v1/relux/oversight`
  read backing a Work oversight strip + a visible **Blocked / Failed** column with run / continuation
  controls (continue, cancel, retry, open-approval), the `current_prime_continuation_handle` surviving a
  refresh; (2) **inline approval decisions** on the oversight strip; (3) **Work hierarchy + progress v1**
  (sub-work + progress from real orchestration data); (4) **ad-hoc task subtrees v1** (`Task.parent_task`
  populated via `create_task_with_parent`); (5) **Work board status movement v1**
  (`POST /v1/relux/tasks/:id/status`, settable-status allowlist + terminal guard, compact Block / Cancel
  control); (6) **per-subtree run / cost rollup v1** (pure client join `runrollup.ts`, honest "cost
  unavailable" not a fake `$0`); and (7) **dashboard route + dependency-free live-browser click smokes**
  (Relux Approvals rebuilt on the B&W design system, per-route render smoke). (The post-tag reparenting +
  recovery + Inbox line rolled up into **v0.1.29**, below.) All reads/writes hit real kernel
  state. No master-plan safety property is weakened. `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel`; dashboard green; `dashboard-dist` in sync. Every safety property from v0.1.27
  holds.
- **v0.1.27** (2026-06-12) — **autonomy policy + managed-stdio MCP + encrypted local secrets** rollup. The
  `relux-kernel` crate moves `0.1.26` → `0.1.27` and `relux-core` `0.1.25` → `0.1.27` (restoring lockstep —
  `0.1.26` had been cut from a feature commit that bumped only the kernel). First packaged bundle since
  v0.1.26; it rolls up the post-v0.1.26 work into one release: (1) a real **configurable Prime autonomy
  policy** (tool-plan width std 16 / ext 64 / ceil 64, orchestration width + read-only context rounds,
  background-job concurrency) retiring the `MAX_TASK_TOOL_PLAN_STEPS` / `MAX_ORCHESTRATION_STEPS` /
  `MAX_TOOL_ROUNDS` / `MAX_ACTIVE_JOBS` toy caps per `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md`; (2) **Prime
  Agent Loop v1** with a genuine resumable continuation ("keep working" resumes from stored observations,
  not a blind re-run); (3) a second governed **managed-stdio MCP transport** (argv-only, spawn-per-op, same
  gates) plus an operator start/stop/restart/status **lifecycle** with process reuse; (4) a **local secret
  store** + secret-referenced env + confined cwd for managed-stdio servers, the Prime OpenRouter brain key
  moved to a **write-only secret reference** (never plaintext), and Windows **DPAPI encryption at rest** for
  stored secrets (per-value scheme markers, fail-safe plaintext fallback + auto-migration); (5) safe
  read-only **plugin source introspection** ("what is this?" hints, source never executed) + one-click MCP
  hint→register. No master-plan safety property is weakened: MCP stays loopback-only, no downloaded code is
  ever run, secrets are never returned in plaintext or persisted unencrypted, and every tool call flows
  through the SAME permission / risk-approval / grant / audit gates. `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel`; dashboard green; `dashboard-dist` in sync. Every safety property from v0.1.25
  holds.
- **unreleased** — **Governed managed-stdio MCP transport (closes the loopback-HTTP-only gap)** on top of
  the MCP hint→register slice, continuing the §8.2/§18 + `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9
  ("P2 — MCP tool support") line, built reference-first against Hermes
  `reference/hermes-agent-main/hermes_cli/mcp_config.py` (`cmd_mcp_add`/`_probe_single_server` — a server is
  `{"url"}` HTTP or `{"command","args","env"}` stdio, spawned + probed by connect→`tools/list`→disconnect)
  and `crates/relix-runtime/src/nodes/tool/mcp_stdio.rs` (the prior async stdio MCP client: `kill_on_drop`,
  `initialize` + `notifications/initialized`, drain notifications until a matching-id response, honest error
  on every failure) — `docs/reference-driven-development.md` (BINDING; mapping in `docs/mcp.md` "Managed
  stdio MCP servers"). **What closes:** many real MCP servers are stdio commands, and Relux previously told
  the operator to stand up a loopback HTTP shim themselves. Now the registry supports a **second safe
  transport** — `relux_core::McpTransport::ManagedStdio` (`McpServerConfig.command`/`args`, serialized,
  no secret) — registered via the SAME `POST /v1/relux/mcp/servers` route and the SAME
  list/discover/classify/invoke/remove surfaces. **Safe by construction:** the command+args are validated
  (`relux_core::validate_stdio_command`) and passed argv-only to `std::process::Command` (no shell, no
  metacharacter-injection surface; a space is allowed so a full path works); no env is stored (it would
  carry secrets), no `cwd` is overridden, and a bypass/danger-flag denylist
  (`--dangerously-skip-permissions` / `--dangerously-bypass-approvals-and-sandbox` / `--yolo`) is refused.
  **Operator-controlled lifecycle:** registration **never spawns** — the command runs only on a later
  operator-driven Discover/invoke; the blocking client (`crates/relux-kernel/src/mcp_stdio.rs`)
  **spawns-per-operation, reaps on drop** (no lingering daemon), bounds every request by the per-call
  timeout (child killed on expiry), caps each stdout line (4 MiB), and drains a **bounded, secret-redacted
  stderr tail** into the failure message. The `tools/list`/`tools/call` results reuse the SAME bounded,
  sanitized, secret-redacted shapers as the HTTP client (no second result path); a `tools/call` `isError`
  is an honest failure, never a fabricated success; the raw JSON-RPC envelope is never returned. **Same
  gates:** a discovered stdio tool maps to the identical `mcp:<server>` namespace + `tool:mcp-<server>:<verb>`
  permission + fail-closed Medium+Required default and invokes through the unchanged
  `call_tool`/`invoke_tool`/approval/grant/audit path — registering or running a server auto-approves
  nothing. **Plugin hints prefill (advisory):** a detected stdio `{command,args}` now pre-fills a reviewable
  managed-stdio registration draft (`mcp_proposal` `suggested_transport`/`detected_command`/`detected_args`),
  executing nothing on import. **UI:** the MCP servers form gains a Transport selector + command/args fields
  (pre-checked with the same argv-only rules in `apps/dashboard/src/plugins.ts`); the list shows transport +
  `transport_display` + Discover/Remove (Resources hidden for stdio — tools-only bridge). **Honest gaps:** no
  long-lived daemon, no env/`cwd`, resources are HTTP-only. No release cut; no master-plan safety property
  weakened (no remote host dialed, no downloaded code run on import, no secret stored). `cargo test` +
  `clippy` clean on `relux-core`/`relux-kernel` (new tests: stdio config validate/serialize both transports,
  shell-metachar/empty-command/too-many-args/bypass-flag rejection, a REAL subprocess fixture
  `src/bin/relux_mcp_test_server.rs` driven end-to-end via `tests/mcp_stdio.rs` — spawn→initialize→list→call,
  `isError` honest, unknown-tool clean, kernel registry discovery; route accepts/validates the stdio body);
  dashboard typecheck/build/tests green (368), committed bundle rebuilt.
- **unreleased** — **Managed-stdio MCP server lifecycle v1 (operator Start/Stop/Restart + process reuse;
  closes the "no long-lived daemon" gap)** on top of the governed managed-stdio transport, same §8.2/§18 +
  `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9 line, built reference-first against Hermes
  `reference/hermes-agent-main/hermes_cli/mcp_config.py` (the SDK keeps the MCP client **connected** between
  `list_tools`/`call_tool`; `_probe_single_server` connect→list→disconnect is the one-shot probe) and the
  prior `crates/relix-runtime/src/nodes/tool/mcp_stdio.rs` posture — mapping in `docs/mcp.md` "Managed-stdio
  process lifecycle". **What closes:** the prior slice was spawn-per-operation only and listed "no long-lived
  daemon" as a gap, so an expensive-to-start server paid spawn+handshake on every call. **What ships:** a
  process-global managed pool (`crate::mcp_stdio::pool()` / `ManagedPool`) that lives **outside** the
  serializable `KernelState` (a live OS process is not snapshot state) — the registry stays the source of
  truth for *what* is registered, the pool owns *whether it is running*. The kernel drives it
  (`start_mcp_stdio_server`/`stop…`/`restart…`/`mcp_stdio_status`/`mcp_stdio_statuses`, each validated
  against the registry + audited `mcp:server_start`/`_stop`/`_restart`) via new routes
  `GET /v1/relux/mcp/servers/status`, `GET …/:id/status`, `POST …/:id/{start,stop,restart}`. **Reuse:** while
  a managed process is running, Discover (`tools/list`) and gated invocation (`tools/call`) **reuse the one
  `initialize`d process** — monotonic JSON-RPC ids, responses matched to their request id (a stale reply /
  notification is drained, never confused), per-call timeout, process-death detection; when nothing is
  running the same operations fall back to the safe spawn-per-operation transport (so Discover/invoke never
  silently fail). **Honest status** (`relux_core::ManagedStdioStatus`: `stopped`/`starting`/`running`/`failed`,
  pid, started-at, redacted `last_error`, `tools_count`, bounded secret-redacted log tail): a failed start is
  a `failed` status with the reason (never a fabricated `running`); a process that dies / a fatal transport
  error tears the process down and marks `failed` (never a fabricated success); an application error (JSON-RPC
  `error` / `tools/call` `isError`) leaves the process healthy and reusable. **Same safety contract** as the
  per-operation transport (argv only — never a shell; no env stored; no `cwd` override; no bypass/danger
  flag; bounded process + redacted log memory; killed + reaped on stop/restart/drop/shutdown/registration
  removal — a removed server never leaves a daemon behind); nothing auto-started, nothing auto-approved.
  **UI:** the Plugins page adds a per-stdio-server **Process** control row (`ManagedStdioControls` +
  `managedStdioStatusBadge`) showing the live status (state/pid/start-time/tools-count/redacted
  last-error+log-tail) and Start/Stop/Restart. **Honest gaps:** a timed-out / transport-failed call tears the
  warm process down (avoids a desynced pipe); still no env/`cwd`; resources still HTTP-only; status is polled,
  not pushed. No release cut; no safety property weakened (no remote host dialed, no downloaded code run, no
  secret stored). `cargo test` + `clippy` clean on `relux-core`/`relux-kernel` (new tests: pool
  start/list/call/stop lifecycle against the REAL subprocess fixture, **one-process reuse proven via a
  per-process pid+counter** `whoami` tool, restart spawns a new pid, a `crash` tool proves process-death →
  `failed`+reason, reuse requires an explicit start, a failed start is an honest `failed` status; kernel
  lifecycle through the registry incl. HTTP/disabled/unknown refusals + remove-stops-the-process; the
  lifecycle routes incl. 400/404 mappings; `ManagedStdioStatus` serde shape); dashboard typecheck/build/tests
  green (369), committed bundle rebuilt.
- **unreleased** — **Local secret/env/cwd foundation for managed-stdio MCP servers (closes the "no env / no
  `cwd`" gap)** on top of the managed-stdio transport, same §8.2/§18 + §17.5, built reference-first against
  Hermes `reference/hermes-agent-main/hermes_cli/mcp_config.py` (a stdio server is `{"command","args","env"}`;
  the per-server key lives in a SEPARATE `~/.hermes/.env` and is REFERENCED via `${ENV}`; `cmd_mcp_test` masks
  the value, never prints it) and `crates/relix-web-bridge/src/secrets.rs` + `os_secure.rs` (separate
  permission-restricted file, atomic write, no-plaintext-return, tail-redacted preview) —
  `docs/reference-driven-development.md` (BINDING); mapping in `docs/mcp.md` "Local secrets & environment".
  **What closes:** the prior managed-stdio slices listed "no env / no `cwd`" as a gap, so a server needing a
  token or a working directory could not run through the managed transport. **What ships:** (1) a local,
  file-backed **secret store** (`relux-kernel::secret_store`, `secrets.json` hardened to owner-only — POSIX
  `0600` / Windows `icacls`) that lives **outside** the kernel snapshot (like the managed pool), with
  set/list/delete that **never return a plaintext value** (only a redacted `relux_core::SecretStatus` preview)
  and a single internal `resolve` used solely at spawn; routes `GET /v1/relux/secrets`,
  `PUT /v1/relux/secrets/:name { value }` (write-only), `DELETE …/:name`. (2) **Secret-referenced `env`** on a
  managed-stdio `McpServerConfig` (`env: { "<ENV_VAR>": { "secret": "<name>" } }`, `relux_core::McpEnvRef`) —
  the config stores only NAMES, never values; resolved into the child env at Start/Restart/spawn-per-op and
  handed straight to `Command::env`, never serialized/logged/retained; a missing secret is a clean `failed`
  status naming the KEY (never a value). (3) A confined optional **`cwd`** validated fail-closed
  (`validate_managed_cwd`: no `..`, must exist + be a dir + canonicalize INSIDE the configured safe MCP
  workspace root `RELUX_MCP_WORKSPACE_ROOT`, symlink-escape rejected). **Same safety contract** otherwise
  (argv only — never a shell; no bypass/danger flag; protected/bundled plugins untouched; nothing auto-run on
  registration). **UI:** Plugins page gains a **Secrets & environment** section (write-only value field,
  redacted listing, delete) and the MCP form gains **Environment** (`ENV_VAR=secret_name`) + **Working
  directory** fields, pre-checked with the kernel's rules.

  **Encryption at rest (follow-up slice, shipped).** Each secret now carries a per-value **scheme marker**
  (`relux-kernel::secret_cipher`): on **Windows** the value is sealed with **DPAPI, CurrentUser scope**
  (`CryptProtectData` via PowerShell's `ProtectedData` — the same `unsafe`-free shell-out posture as
  `os_secure`'s `icacls`; plaintext rides stdin/stdout, never an argv), stored as `base64(blob)` under
  scheme `dpapi_current_user`; on other OSes / when DPAPI is unavailable it stays **permission-hardened
  plaintext** under the honestly-named `plaintext_file_v1`. Reads **dispatch on the stored scheme** (mixed
  schemes read correctly; a value sealed on a host another can't unseal **fails closed**, naming the secret +
  scheme, never the value); a legacy plaintext file **auto-migrates** (re-seals) to the active scheme on load,
  fail-safe (a re-seal failure leaves the entry untouched). `SecretStatus` exposes the `scheme` (never the
  value); `list`/`status` never decrypt (preview precomputed at set time). **Remaining gap:** no
  macOS/Linux keychain yet (those stay file-permission-hardened plaintext); DPAPI is CurrentUser-scoped
  (protects against another user / an offline disk image, not against code already running as the same user);
  rotation is manual (re-set re-seals); env is per-process (Restart to pick up a changed secret).
  No release cut; no safety property weakened (no
  remote host dialed, no downloaded code run, **no plaintext secret stored in the config/snapshot/API**).
  `cargo test` + `clippy` clean on `relux-core`/`relux-kernel` (new tests: secret set/list/delete never return
  plaintext, redacted preview, hardened-file round-trip; spawn-Command env/cwd injection + a REAL-subprocess
  `env_probe` proving the child receives the resolved secret via a value hash without printing it; missing
  secret → clean failed status naming the key; cwd traversal/outside-root rejected, inside-root accepted);
  dashboard typecheck/build/tests green (371), committed bundle rebuilt.
- **unreleased** — **Background-job concurrency folded into the autonomy policy (retires the hidden
  `MAX_ACTIVE_JOBS = 4`)**, the FINAL LATER item from `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` promoted to
  FIXED, finishing the artificial-constraint pass and continuing the autonomy-policy line (§10.5/§17.1).
  Built reference-first against the Hermes api-server's configurable `max_concurrent` admission knob
  (`reference/hermes-agent-main/.plans/openai-api-server.md` — a named, raisable concurrency limit, not a
  hidden wall) — `docs/reference-driven-development.md` (BINDING). **What changes (safe, bounded):** the
  async `run-async` orchestration-job fleet cap was a hidden `const MAX_ACTIVE_JOBS: usize = 4` in
  `server.rs` — a **real** resource guardrail (each active job drives live adapter processes on its own
  OS thread) but a fixed, invisible one. It becomes two new `relux_core::PrimeAgentPolicy` fields —
  `max_active_jobs` (standard **4**, the value the constant held) / `extended_max_active_jobs` (**16**) —
  clamped to the absolute hard backstop `MAX_ACTIVE_JOBS_CEIL` (**64**); even extended is bounded, never
  unlimited, so a request burst can never spawn unbounded workers. `JobRegistry::start` now takes the
  resolved cap as an argument (the registry no longer hard-codes a number); the `run-async` route reads
  it via `PrimeAgentPolicy::active_jobs(extended)` and a request may opt into the higher profile with
  `{"extended": true}`. The over-limit `429` now **names the configured limit and how to raise it**
  (extended retry / which policy field / the route), never a generic "too many". Surfaced on
  `/v1/relux/prime/agent-policy` (GET resolves per profile; PUT/PATCH clamps), the `prime agent-policy
  configure` CLI (`--max-active-jobs` / `--ext-max-active-jobs`), and the dashboard Prime Autonomy Limits
  panel (new **Active jobs** row + `jobs std/ext active` chip). No release cut; no safety property
  weakened (the concurrency guardrail stays finite + clamped, just configurable). `cargo test` + `clippy`
  clean on `relux-core`/`relux-kernel`; dashboard typecheck/build/tests green (tests pin: the configured
  admission limit is enforced, a raised limit admits strictly more than the old hard-coded 4, the
  over-limit message names the limit, route/CLI serialization clamps the fields, no regression to
  cancellation / run logs / tool gates). With this, **no artificial toy-cap items remain open** in the
  relux-\* product layer.
- **unreleased** — **Orchestration width + read-only context rounds folded into the autonomy policy
  (one operator dial)**, the last two LATER items from `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` promoted
  to FIXED, continuing the autonomy-policy line (§10.5/§17.1) and built reference-first against Hermes
  `agent/iteration_budget.py` (a tunable bound, not a tiny constant) — `docs/reference-driven-development.md`
  (BINDING). **What changes (safe, bounded):** four new `relux_core::PrimeAgentPolicy` fields —
  `max_orchestration_steps` (standard **16**) / `extended_max_orchestration_steps` (**64**), clamped to
  the shared `MAX_ORCHESTRATION_STEPS_CEIL` (**64**); and `max_context_rounds` (standard **8**, aligned
  with `MAX_TOOL_ROUNDS`) / `extended_max_context_rounds` (**32**), clamped to `MAX_CONTEXT_ROUNDS_CEIL`
  (**64**). The pure planner now takes the width as an argument (`plan_orchestration_with_limit`; bare
  `plan_orchestration` keeps the default constant), and BOTH authoritative create-paths — the
  deterministic `prime_orchestrate` and the brain `reconcile_orchestration_slots` — read the SAME
  resolved `orchestration_steps(false)` width, so they can never drift; the preview route resolves the
  same width; an over-width goal's overflow note **names the active limit and how to raise it**. The
  read-only `ContextLoop` / up-front `execute_requested_reads` take the resolved `context_rounds(false)`
  budget (threaded from the server preview block into the observe-then-act `DecisionLoop` and the
  sidecar loop), preserving the no-progress / repeat early-stop; the parse path bounds the request list
  at the ceiling so a raised/extended list still reads back. Surfaced on `/v1/relux/prime/agent-policy`
  (GET resolves per profile; PUT/PATCH clamps), the `prime agent-policy configure` CLI
  (`--max-orchestration-steps`/`--ext-…`, `--max-context-rounds`/`--ext-…`), and the dashboard Prime
  Autonomy Limits panel (new **Orchestration** + **Context loop** rows and chips). No release cut; no
  safety property weakened (every bound stays finite + clamped, just configurable). `cargo test` +
  `clippy` clean on `relux-core`/`relux-kernel`; dashboard typecheck/build/tests green (tests pin:
  standard default 16 / >6 works, smaller configured limit honored + named in the overflow note,
  extended > standard, context loop honors a configured + higher extended budget at resolve-level,
  route/CLI serialization clamps the fields, no regression to chat / tool gating / tool-plan policy).
- **unreleased** — **Artificial-constraint audit + the next toy-cap fixes** continuing the autonomy-policy
  line (§10.5/§17.1), built reference-first against Hermes `agent/conversation_loop.py` /
  `agent/iteration_budget.py` (the "high configurable ceiling, not a tiny constant" precedent). After the
  Prime Agent Loop's toy 3/3 cap was replaced by a configurable policy, this slice sweeps the rest of the
  **relux-\*** product layer for the same class of mistake and records the result in
  `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` (FIX NOW / KEEP-with-reason / LATER). **What changes (safe, bounded):**
  (1) the orchestration step cap was a function-local `MAX_STEPS = 6` duplicated as a second literal in
  `prime_orchestration_slots.rs`; it is now a single named `relux_core::MAX_ORCHESTRATION_STEPS = 16` both
  paths reference (no drift), so a real multi-part goal is no longer truncated to six briefs — overflow is
  still reported in an honest note, never dropped. (2) Prime's **read-only** context loop bound
  `MAX_TOOL_ROUNDS` was a toy `4`; raised to `8` (Hermes' own default is 90), still finite, still stopping
  early on a repeated/no-progress read. **What is deliberately KEPT** (documented as real guardrails, not toy
  caps): the clamped `PrimeAgentPolicy` ceilings (already configurable), the echo fixture's demotion to
  internal-only (`is_internal_plugin` — verified, not rebuilt), `create_agent`'s least-privilege grant, the
  MCP loopback/size bounds, and every char/byte/HTTP-body clamp. **LATER** (recorded with exact next steps):
  folding the orchestration width and `MAX_TOOL_ROUNDS` into operator-configurable policy fields, and lifting
  `MAX_ACTIVE_JOBS`. No release cut; no safety property weakened. `cargo test` +
  `clippy` clean on `relux-core`/`relux-kernel` (orchestration/​slots/​context-loop/​decision tests pin the
  raised-but-bounded caps via the named constants).
- **unreleased** — **Configurable tool-plan step policy (retires the hidden `MAX_TASK_TOOL_PLAN_STEPS = 5`)**,
  the next LATER item from `docs/ARTIFICIAL_CONSTRAINT_AUDIT.md` promoted to FIXED, continuing the
  autonomy-policy line (§10.5/§17.1) and built reference-first against Hermes `agent/iteration_budget.py`
  (a tunable bound, not a tiny constant) — `docs/reference-driven-development.md` (BINDING). **What changes
  (safe, bounded):** the toy `5` becomes two new `relux_core::PrimeAgentPolicy` fields — `max_tool_plan_steps`
  (standard, default **16**, aligned with `MAX_ORCHESTRATION_STEPS`) and `extended_max_tool_plan_steps`
  (default **64**) — clamped to the absolute hard backstop `MAX_TASK_TOOL_PLAN_STEPS_CEIL` (**64**, also the
  read-path bound). `TaskToolPlan::validate_with_limit(max)` is the new operator-facing validator; the no-arg
  `validate()` keeps a conservative static default (`MAX_TASK_TOOL_PLAN_STEPS`, now **16**) for tests/CLI. The
  configured limit is applied consistently at the Prime tool-plan proposal, the UI-created tool-run task route,
  and the `/v1/relux/prime/agent-policy` route + `prime agent-policy configure` CLI; the dashboard Prime
  Autonomy Limits panel gains a **Tool plan** row. An over-limit plan is an honest `400` / blocking issue that
  **names the limit** — never silently truncated. No release cut; no safety property weakened (the bound is
  still finite and clamped, just configurable). `cargo test` + `clippy` clean on `relux-core`/`relux-kernel`;
  dashboard typecheck/build/tests green (tests pin: standard permits >5, over-limit named, extended > standard,
  static default safe, proposal + create-route honor the configured limit).
- **unreleased** — **Resumable Prime agent-loop continuation (the real "keep working")** on top of the
  configurable autonomy policy, continuing the §10.5/§17.1 line, built reference-first against Hermes'
  `agent/conversation_loop.py` (`run_conversation(conversation_history=…)` seeds `messages =
  list(conversation_history)` so a resumed turn carries the prior `role:"tool"` results and does not
  re-run them; session persisted in `agent._session_db` keyed by `session_id`) and openclaw's
  consume-once exec-approval handoff, per `docs/reference-driven-development.md` (mapping in
  `docs/mcp.md`, "Resumable continuation"; `docs/REFERENCE_CODE_MAP.md`). No release cut; no
  master-plan safety property is weakened. **What changes:** the prior continuation was a *fresh,
  audited turn that re-ran the original request* under the higher profile — it duplicated tool calls
  and felt fake. It is replaced by a REAL resume: when a bounded agent-loop turn pauses with work
  still to do (a configured ceiling hit, or a gated tool waiting on approval), the kernel persists a
  bounded, redacted `relux_core::PrimeAgentContinuation` (the original request, the profile used, the
  already-gathered observations + their call signatures, the pause reason, and any staged approval id)
  keyed by conversation in the snapshot, and stamps a `prime_continuation` handle (a stable `cont_NNNN`
  token) on the response. `POST /v1/relux/prime/agent/continue` validates the token (stale / unknown /
  expired **fails closed**), CONSUMES the record, seeds `AgentLoop::resume` with the prior
  observations, and continues under a FRESH per-turn budget — the brain sees the prior results and
  **skips already-completed calls** (by call signature, bounded self-correction), so it proceeds PAST
  where it stopped instead of re-running blind. **Approval resume is automatic:** the unchanged
  approval routes run the gated tool once, and `execute_approved_tool_invocation` folds the real
  result into the waiting continuation (clearing the pause); the next "Keep working" resumes with it
  in context. Denying drops the continuation. **Bounded/safe:** one record per conversation, a TTL
  (`PRIME_CONTINUATION_TTL_SECS`), `MAX_PRIME_CONTINUATIONS` overall, `MAX_CONTINUATION_STEPS` steps
  each, every step secret-redacted; the continuation grants NO authority (every resumed execution
  flows through the unchanged `prime_invoke_tool` gate); normal chat / frustration / vague ideas still
  never create a continuation (a continue only resumes an existing paused loop). **UI:** the dashboard
  "Keep working (extended)" button now calls the continuation route with the token (not a re-sent
  message) and shows a compact "⏸ paused · <reason> · <N> gathered" chip; an approval-waiting
  continuation tells the operator to approve first. **v2 gaps (honest):** no live streaming, no
  parallel tool branches, the brain re-reasons from the carried observations (its intermediate
  reasoning tokens are not carried), and the loop still never picks tools the user did not explicitly
  request. `cargo test` + `clippy` clean on `relux-core`/`relux-kernel` (new tests: resumed loop feeds
  prior observations + skips duplicate calls, a fresh budget proceeds past the prior limit, repeated
  re-picks of a finished call stop, create/peek/take/fold/snapshot-roundtrip + bounded steps, stale /
  unknown / mismatched / expired tokens fail closed, approved result folds in, denied drops it, the
  continue route gates an empty id / Local brain / unknown token, normal chat carries no continuation).
  Dashboard typechecks, builds, its 358 tests pass, and the committed bundle was rebuilt.
- **unreleased** — **Configurable Prime autonomy policy (replaces the toy v1 loop caps)** on top of the
  Prime Agent Loop, continuing the §10.5/§17.1 line, built reference-first against Hermes'
  `agent/iteration_budget.py` (`IterationBudget(max_total)` — a configurable per-agent budget, parent
  `max_iterations` default **90**, `delegation.max_iterations` default **50**) and `cli-config.yaml.example`,
  per `docs/reference-driven-development.md` (mapping in `docs/mcp.md`, "Prime Agent Loop"; `docs/REFERENCE_CODE_MAP.md`).
  No release cut; no master-plan safety property is weakened. **What changes:** the agent loop's hard-coded
  `MAX_AGENT_TOOL_CALLS = 3` / `MAX_BRAIN_ROUNDS = 3` made Prime feel artificially limited. They are replaced
  by an operator-set `relux_core::PrimeAgentPolicy` with two profiles — a practical **standard** default
  (12 tool calls / 18 brain rounds / 180s wall-clock) and a higher **extended** profile (64 / 96 / 1800s) for
  user-initiated long work — resolved per turn into `relux_kernel::AgentLimits`. The loop reads these instead
  of constants (`prime_agent_loop.rs`), enforces an optional wall-clock deadline (`mark_deadline_exceeded`,
  the kernel owns the clock), and when a ceiling is reached returns `AgentOutcome::LimitReached(LimitKind)` so
  the turn **names the exact limit, shows what it gathered, and offers a one-click "Keep working (extended)"
  continuation** — never a fabricated "done". The extended profile is selected when the user explicitly asks
  to keep working (`prime_wants_extended_work`, a fallback keyword rail that only raises the ceiling for an
  already-`ToolInvocation` turn). **Why bounded, not infinite:** every policy field is clamped
  (`PrimeAgentPolicy::clamped`: ≤512 calls / ≤1024 rounds / ≤24h) — an operator can scale up for serious work
  but cannot set "infinite"; a literal unbounded loop is unsafe (runaway cost, never yields), so the model is
  operator-controlled high limits + an explicit, auditable continue. Approvals still pause; high-risk tools
  never auto-run; normal chat / frustration / vague ideas still never enter the loop. **Storage/serve:**
  persisted in the kernel snapshot/store (clamped on load); served at `GET/PUT/PATCH /v1/relux/prime/agent-policy`
  (response carries the resolved standard/extended limits); `relux-kernel prime agent-policy <status|configure>`
  CLI. **UI:** a compact **Prime Autonomy Limits** panel (Health → Prime Brain) with std/ext chips + editable
  rows; the "Keep working (extended)" affordance rides the existing `suggested_actions` chat buttons.
  **Continuation model (at the time of this slice):** a continuation was a fresh, audited turn that re-ran the
  original request under the extended profile — SUPERSEDED by the "Resumable Prime agent-loop continuation"
  entry above, which makes it a real resume from the already-gathered observations. `cargo test` + `clippy`
  clean on `relux-core`/`relux-kernel` (new
  tests: default policy is not the toy 3, configured high tool limit runs >3, extended uses higher limits than
  standard, brain-round + duration ceilings reported as limits, extended-work cue detection, policy persists +
  clamps through the snapshot, the `/agent-policy` route serves + clamps). Dashboard typechecks, builds, its
  358 tests pass, and the committed bundle was rebuilt.
- **unreleased** — **Prime Agent Loop v1 (bounded think → tool → observe → respond, in chat)** on top of
  the chat-staged-approval slice, continuing the §9/§10.5/§17.1 line, built reference-first against Hermes'
  `agent/conversation_loop.py` (`run_conversation` bounded `while api_call_count < max_iterations` tool
  loop + `valid_tool_names` gate + tool-result observation feedback) and openclaw's fail-closed
  mutate/approval classifiers per `docs/reference-driven-development.md` (mapping in `docs/mcp.md`, "Prime
  Agent Loop v1"; `docs/REFERENCE_CODE_MAP.md`). No release cut; no master-plan safety property is weakened
  — no gate is bypassed, nothing is auto-approved, and the loop invents no second security model.
  **What closes:** the single explicit invocation ran ONE named tool and stopped. Now, on an explicit
  tool-request turn (`classify_intent` → `ToolInvocation`, the SAME safety wall — normal chat / profanity /
  vague ideas / Q&A never enter), a configured brain may **call an allowed tool, observe its real output,
  and continue**, chaining up to `MAX_AGENT_TOOL_CALLS = 3` tools across `MAX_BRAIN_ROUNDS = 3` rounds and
  folding the observations into a useful final answer. The pure, unit-tested driver
  (`crates/relux-kernel/src/prime_agent_loop.rs` — `AgentLoop`, the live `AgentTool` catalog =
  `valid_tool_names`, `interpret_agent_reply` with off-catalog self-correction, redacted/bounded
  `AgentObservation`) executes each pick through the UNCHANGED single-invocation gate (`state.rs`
  `prime_agent_step` → `prime_invoke_tool`): a `Ready` (or allow-always-granted) tool runs and is observed;
  a gated tool with no grant returns the EXISTING staged approval card and the loop **pauses** (nothing
  ran); a missing/unrunnable/unknown tool fails closed honestly. The off-lock-brain / short-locked-exec
  orchestration is `server.rs` `drive_prime_agent_loop` (the kernel lock never spans a brain call). **UI:**
  a compact `tool_trace` chip strip (`relux_core::PrimeToolTrace` → `Prime.tsx` `ToolTrace`) for a chain; a
  single tool keeps its existing result render; no raw CLI JSON / transport envelope reaches the user.
  **Fail-closed + honest:** the catalog offers only `Ready`/`NeedsApproval` tools the agent can run (the
  brain cannot pick a tool it lacks permission for or that has no runtime); a stale/off-catalog pick is
  refused; an errored run is an `ok:false` observation, never a fabricated success. **v2 gaps (honest):**
  no automatic brain resume in the same turn after an approval is granted (the bound call runs once via the
  existing routes; the brain resumes on the next message, now grant-covered); no live streaming, branching,
  or parallel tools; the brain may not pick tools the user did not explicitly request. `cargo test` +
  `clippy` clean on `relux-core`/`relux-kernel` (incl. new targeted tests: greeting/frustration never loop,
  low-risk tool executes + observation grounds the answer, gated tool pauses with the card, allow-always
  grant runs inside the loop, unknown tool fails closed, tool calls bounded to the cap, MCP tool
  participates; plus kernel `prime_agent_step` tests: granted run yields an observation, gated-no-grant
  stages the card, unknown fails closed, catalog lists a runnable MCP tool). Dashboard typechecks, builds,
  and its tests pass; the committed bundle was rebuilt. Every safety property from the prior slice holds.
- **unreleased** — **chat-staged tool approvals (gated chat tool calls become usable)** on top of the
  single-invoke slice, continuing the §9 ("P2 — MCP tool support") line, built reference-first against
  openclaw's `src/acp/permission-relay.ts` (the canonical allow-once / allow-always / deny decision model
  + stable approval-id correlation key) and `src/acp/approval-classifier.ts` (unknown → fail-closed, never
  auto-approve), per `docs/reference-driven-development.md` (mapping in `docs/mcp.md`, "Chat-staged
  approval"; `docs/REFERENCE_CODE_MAP.md`). No master-plan safety property is weakened — no gate is
  bypassed, nothing is auto-approved, and no dangerous/bypass flag is ever passed to an adapter.
  **What closes:** a gated (`needs_approval`) tool an operator names in Prime chat used to **dead-end** in
  an honest-but-useless refusal. Now `prime_invoke_tool`'s `NeedsApproval` arm (1) runs the call directly
  when a standing **allow-always grant** already authorizes the exact `(agent, plugin, tool, permission,
  risk)` (the §7.4 grant fast path, via `matching_persistent_grant_id` → `invoke_tool`), or (2) **stages a
  pending per-call approval** through the EXISTING `request_tool_invocation_approval` machinery (re-checks
  the permission, re-confirms approval is required, bounds the args, binds the consume-once
  `PendingToolInvocation`) and returns `disposition = awaiting_approval` carrying
  `PrimeTurn.pending_tool_approval` (`relux_core::PrimeToolApprovalRequest`: approval id, `<plugin>/<tool>`
  label, `mcp`/`plugin` source + server, lowercase risk, reason, **secret-redacted** args preview, required
  permission, `allow_always_supported`). **Nothing runs** at stage time. **UI:** a compact chat
  `ApprovalCard` (`apps/dashboard/src/pages/Prime.tsx`) offers the three openclaw decisions wired to the
  EXISTING routes ONLY — "Approve & run" (`/approvals/:id/decide` approved → `/execute`), "Allow always"
  (`/approvals/:id/allow-always` → `/execute`), "Deny" (`/approvals/:id/decide` rejected, which drops the
  bound invocation) — no parallel security path, no new backend. **Fail-closed + honest:** a missing /
  unreachable / disabled / unregistered tool still surfaces a clean `tool_error` (it never stages an
  approval); normal chat / profanity / vague ideas / deliberative questions never reach the arm
  (`is_chat_guarded`), so they never stage an approval. `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel` (incl. new targeted tests: gated MCP + gated plugin chat invocation each
  stage a pending approval not a dead refusal, allow-always grant runs directly, the staged approval runs
  once through the existing execute route, deny drops the binding/stays safe). The dashboard typechecks,
  builds, and its tests pass; the committed bundle was rebuilt. Every safety property from v0.1.25 holds.
- **unreleased** — **single MCP tool invocation from Prime chat** on top of v0.1.25, continuing the §9
  ("P2 — MCP tool support") line from `docs/HERMES_OPENCLAW_DEEP_AUDIT.md`, built reference-first against
  the vendored Hermes one-off tool-call path (`agent/conversation_loop.py` valid-tool gate +
  `tools/mcp_tool.py` `tools/call` result shaping) and the openclaw `mcp:<serverId>:<toolName>` ref
  (`src/tools/execution.ts`) + fail-closed mutate default (`src/agents/tool-mutation.ts`) per
  `docs/reference-driven-development.md` (mapping in `docs/mcp.md`, "Invocation"). No release cut; no
  master-plan safety property is weakened. **What closes:** the single-tool Prime invoke path
  (`PrimeAction::InvokeTool` → `prime_invoke_tool`) was plugin-only; it now resolves an explicit
  `mcp:<server>/<tool>` reference the user names in chat ("use mcp:loopback/status.summary",
  "call mcp:fs/search with {…}", or a bare `mcp:fs/search`). Recognition reuses the SAME
  `parse_tool_request` resolver as the plan path (`classify_intent` → `ToolInvocation` for a single MCP
  ref; gated so a question/musing/insult never invokes — `is_chat_guarded`). Grounding reuses the SAME
  off-lock live catalog (`live_tool_catalog`, fed by the already-existing `discover_proposal_mcp_catalog`
  prefetch — the `mcp:`-token gate in `server.rs` already covered single-ref messages). Execution reuses
  the SAME gated `invoke_tool` (permission → risk/approval + per-call/allow-always grant → audit), and
  the SAME shaped result (text under `result`, never the raw JSON-RPC envelope). **Fail-closed + honest:**
  an unclassified/Medium+Required MCP tool is `needs_approval` and is NEVER auto-run from chat (the reply
  names the existing classify / allow-always-grant / per-call-approval routes); a missing tool, an
  unreachable/disabled/unregistered server each surface a clean, MCP-aware `tool_error` (no blank page,
  no raw JSON). The frontend needs no change — the existing `invoked_tool` / `tool_output` / `tool_error`
  fields already render the `mcp:<server>/<tool>` source label + shaped result cleanly. **Multi-step
  plans stay inert** (unchanged proposal path, still a reviewable card, still operator-click to commit).
  Raw reference reading recorded in `docs/REFERENCE_CODE_MAP.md`. `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel` (incl. 6 new targeted tests: single-ref classification, classified-tool
  run through the gates with shaped result, approval-gate no-auto-allow, missing-tool + unreachable-server
  fail-closed, normal-chat-with-catalog-present never invokes). Every safety property from v0.1.25 holds.
- **v0.1.25** (2026-06-12) — **run-driven multi-tool plans** on top of v0.1.24, continuing the §9
  ("P2 — MCP tool support") line from `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (Next-slice item 5), built
  reference-first against the vendored Hermes `tool_calls` loop and the openclaw `buildToolPlan`
  validate-whole-plan-up-front fail-closed posture per `docs/reference-driven-development.md` (full
  mapping in `docs/mcp.md`, "Run-driven multi-tool plan"). No master-plan safety property is weakened:
  MCP stays **loopback-only**, no downloaded code is ever run, secrets are never persisted or returned,
  and every step still flows through the SAME permission / risk-approval / grant / audit gates a real
  plugin tool uses, with the same `mcp_tool_call*` transcript events. The brain never picks a step —
  the plan is operator-authored and fixed at task creation. **Bounded multi-tool plan:** a `Task` input
  may carry a `tool_plan` of ≤ 5 `{ plugin, tool, args }` steps run sequentially in one local-prime run
  through the gated `call_tool` chokepoint (`relux-core` `TaskToolPlan` + `parse_task_tool_plan` +
  `validate`: non-empty, ≤ 5 steps, per-step non-empty plugin/tool, per-step args size-bounded),
  stopping on the first failure/denial (run + task fail honestly, no partial-success lie);
  `execute_local_run` emits a compact step-count completion summary; `POST /v1/relux/tasks` accepts the
  optional directive (strictly validated, mutually exclusive with `tool_call`, honest `400`).
  **Operator UI:** a compact Plugins → Tools "Create a tool-run task" form (title + 1–5 steps, each a
  discovered tool + optional JSON args) posts a `tool_call` (one step) or `tool_plan` (two-or-more) over
  the existing endpoint with a React-free, fail-closed payload builder (`toolruntask.ts`) that warns
  honestly when a gated tool needs a standing grant. **Live discovery in the picker:** the picker merges
  installed plugin tools with tools discovered live from each enabled MCP server
  (`reluxMcp.list` + `reluxMcp.tools`, keyed `mcp:<server>`), gating via `toolReadiness`, surfacing a
  warning on failed discovery and an info note on a disabled server rather than silently dropping it; the
  Plugins MCP copy now reflects that a discovered tool is callable through the standard gates.
  **Live MCP tools in Prime plan PROPOSALS:** Prime's inert multi-tool-plan preview
  (`KernelState::build_tool_plan_proposal`) now grounds against a SHARED, read-only catalog
  (`live_tool_catalog`) of installed plugin tools PLUS the live MCP-discovered tools of every
  enabled server — so an `mcp:<server>/<tool>` step (recognized by `parse_tool_request`, mirroring
  openclaw's `mcp:<serverId>:<toolName>` ref) resolves exactly like an installed tool and lands in the
  SAME `mcp:<server>` task `tool_plan` execution path (no second tool system). The bounded `tools/list`
  runs OFF-LOCK in the server (`discover_proposal_mcp_catalog`, injected via `set_proposal_mcp_catalog`)
  so the kernel lock never spans a network read; the preview stays INERT (creates nothing, runs nothing)
  and FAILS CLOSED — an unreachable / disabled / unregistered server grounds `unavailable`, an
  un-advertised tool `unknown`, an unclassified MCP tool `needs_approval` — never a faked runnable step.
  Normal chat / brainstorming / frustration still resolve to no tools and produce no plan. Raw reference
  reading recorded in `docs/REFERENCE_CODE_MAP.md`.
  `cargo test` + `clippy` clean on `relux-core`/`relux-kernel`, dashboard tests + typecheck + build
  green, the tracked `dashboard-dist` bundle rebuilt and committed in sync. Every safety property from
  v0.1.24 still holds.
- **v0.1.24** (2026-06-12) — **MCP surface deepening** on top of v0.1.23, continuing the §9
  ("P2 — MCP tool support") line from `docs/HERMES_OPENCLAW_DEEP_AUDIT.md`, built reference-first
  against the vendored Hermes (`tools/mcp_tool.py`, `hermes_cli/mcp_config.py`) and the legacy
  `relix-runtime` streamable-HTTP client per `docs/reference-driven-development.md` (full mapping in
  `docs/mcp.md`). No master-plan safety property is weakened: MCP stays **loopback-only**, no
  downloaded code is ever run, secrets are never persisted or returned, and every MCP tool call flows
  through the SAME permission / approval / grant / audit gates a real plugin tool uses. **Session
  continuity:** the kernel captures the `Mcp-Session-Id` header on `initialize`, validates it to the
  visible-ASCII charset (header-injection guard), and echoes it on the operation's later requests; a
  `404` mid-session triggers one bounded clear-and-re-initialize retry, then fails honestly; the
  session id is in-memory per operation only (never persisted/logged/returned), with no long-lived
  SSE channel and no cross-operation reuse. **Read-only resources:** `relux_core` `McpResource` /
  `McpResourceContent` plus `list_resources` / `read_resource` clients add MCP resources as a
  Prime/operator context source (binary blocks summarized not decoded; text sanitized,
  secret-redacted, clamped), surfaced read-only at `GET /v1/relux/mcp/servers/:id/resources` and
  `.../resources/read?uri=...`, on the Prime `READ_ONLY_TOOLS` allowlist (dialed outside the kernel
  lock, no mutation), and in a dashboard Resources panel (maps Hermes
  `_make_list_resources_handler` / `_make_read_resource_handler`, plus a Relux secret-redact of the
  read body). **Run-transcript visibility:** a run-bound MCP tool call records distinct, bounded,
  secret-redacted `mcp_tool_call` / `mcp_tool_call_denied` / `mcp_tool_call_failed` events
  (`result_summary` redacted + clamped to 500 chars; raw args / `structuredContent` / JSON-RPC
  envelope / session id never in the transcript); manual/approval/out-of-run grant bypasses stay
  audit-only. **First production run path:** a `Task` may carry an operator-named
  `{ tool_call: { plugin, tool, args } }` directive that the deterministic local run
  (`execute_local_run`) routes through the gated `call_tool` chokepoint instead of echo (the brain
  never picks the tool), failing the run/task honestly on a gate refusal; `POST /v1/relux/tasks`
  accepts the optional directive. `cargo test` + `clippy` clean on `relux-core`/`relux-kernel`,
  dashboard tests + typecheck + build green, the tracked `dashboard-dist` bundle rebuilt and
  committed in sync. Every safety property from v0.1.23 still holds.
- **v0.1.23** (2026-06-12) — the **first safe MCP (Model Context Protocol) surface** on top of
  v0.1.22, driven by `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§9, "P2 — MCP tool support") and
  `docs/RELUX_MASTER_PLAN.md` §8.2/§18, built reference-first against the vendored Hermes
  (`tools/mcp_tool.py`, `hermes_cli/mcp_config.py`), the legacy `relix-runtime` streamable-HTTP
  client, and openclaw's `mcp:<server>:<tool>` executor namespace per
  `docs/reference-driven-development.md` (full mapping in `docs/mcp.md`). No master-plan safety
  property is weakened: MCP is **loopback-only**, no downloaded code is ever run, and every MCP tool
  call flows through the SAME permission / approval / grant / audit gates a real plugin tool uses.
  **Loopback registry + live discovery (§9):** an operator registers a loopback-only MCP server
  (`{ id, endpoint, description?, enabled?, timeout_ms? }`, validated with the same
  `validate_loopback_url` rule as the plugin runtime), lists registrations with an honest one-word
  status (no secrets stored/returned), and discovers an enabled server's tools via a live
  `initialize` → `tools/list` handshake mapped into the standard `relux_core::ToolDescriptor` shape
  (`plugin_id = "mcp:<id>"`, enforced permission `tool:mcp-<id>:<verb>`, classified risk,
  `source_kind = "Mcp"`); descriptions are sanitized, clamped, and prompt-injection-scanned
  (advisory), with timeouts/body-caps/≤256-tools bounds mirrored from the loopback-tool runtime. A new
  dashboard MCP UI on the Plugins page drives register → discover → classify. **Gated invocation
  (§9):** MCP tools are first-class tool-invoke citizens routed through the existing `call_tool` /
  `invoke_tool` / per-call-approval / persistent-grant path with the synthetic
  `plugin_id = "mcp:<server>"` — no separate MCP invoke endpoint. A discovered tool defaults to the
  fail-closed `McpToolClassification` (risk `Medium`, approval `Required`) and is refused on the direct
  path until classified low-risk + `Never` (or run via a per-call approval / allow-always grant); on
  invocation the kernel re-checks the `tool:mcp-<server>:<verb>` permission, re-applies the
  risk/approval gate, re-validates the loopback endpoint, runs `initialize` →
  `notifications/initialized` → `tools/call` bounded by the per-call timeout + size caps, shapes and
  sanitizes the result (raw JSON-RPC envelope never returned; `isError` → honest failure), and audits
  the call. Honest limits (stated in `docs/mcp.md`): no stdio servers, no remote/`https`/SSE-subscription
  transport, single-POST subset of streamable HTTP, no OAuth, `tools/*` only. `cargo test` + `clippy`
  clean on `relux-core`/`relux-kernel`, dashboard tests + typecheck + build green, the tracked
  `dashboard-dist` bundle rebuilt and committed in sync. Every safety property from v0.1.22 still holds.
- **v0.1.22** (2026-06-11) — **run-log observability + safe mid-run cancellation** on top of
  v0.1.21, driven by `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§24/§25/§26; §8/§10 P2) and built
  reference-first against the vendored Paperclip/openclaw process-execution paths per
  `docs/reference-driven-development.md`. No master-plan safety property is weakened. **Bounded,
  redacted run-log / tail (§24):** a new `relux_core::RunLog` captures an adapter run's stdout/stderr
  as classified, secret-redacted, per-line-clamped, line-count-capped lines (oldest dropped + counted,
  truncation markers). `GET /v1/relux/runs/:id/logs?since=<seq>` returns the bounded tail (incremental
  past a cursor, full when absent); a run with no captured log returns an empty (not errored) `lines`
  array, only an unknown run id is a 400. The dashboard Work Run Detail adds a Logs / Tail section
  (per-line table, source badge, truncation note, Refresh + poll). **LIVE per-line streaming (§25):** a
  new `relux_core::StreamingRunLog` line-buffers streamed chunks and emits only complete, re-redacted,
  clamped lines while the process runs, enforcing the line cap continuously so a long live stream stays
  bounded (the finalized record is byte-identical to the captured-then-built one); wired through
  `run_adapter_command_streaming` + a kernel `LiveRunLogs` buffer on the off-lock parallel path, so
  lines appear before the run finalizes. The synchronous lock-holding path stays finalize-captured and
  the tail is still POLLED (no SSE/WebSocket push yet — stated honestly). **Safe mid-run cancellation
  (§26):** an `AbortSignal`-style cooperative cancel — `POST /v1/relux/runs/:id/cancel` sets a flag the
  off-lock streaming path observes, kills the child process, and records an honest cancelled result
  inline; a non-cancellable / already-finished run reports honestly (fails closed) rather than
  fabricating a kill. `cargo test` + `clippy` clean on `relux-core`/`relux-kernel`, dashboard tests +
  typecheck + build green, the tracked `dashboard-dist` bundle rebuilt and committed in sync. Every
  safety property from v0.1.21 still holds.
- **v0.1.21** (2026-06-11) — the **first persistent allow-always grant** plus a **Hermes-first
  Prime** re-grounding on top of v0.1.20, driven by `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§23 / §5
  P2) and `docs/prime-processing-audit.md`. No master-plan safety property is weakened. **Persistent
  allow-always grant (§23 / §5 P2):** a standing, explicit, revocable, audited
  `relux_core::PersistentGrant` bound to one exact `(subject, plugin, tool, permission, risk)` lets a
  future matching configured-tool invocation bypass the per-call approval prompt — and ONLY that
  prompt; the subject permission check and the runtime/loopback gate still apply, and the pure
  fail-closed `authorizes_invocation` matcher requires an exact match, so a changed permission or
  escalated risk does not match and the prompt returns. The kernel adds grant/revoke/list +
  `allow_always_from_approval`, the gate in `call_tool` / `invoke_tool` consults a matching grant
  before refusing, `grant:create`/`grant:use`/`grant:revoke` are audited, and grants persist via
  snapshot + SQLite (`next_grant` counter). **HTTP:** `POST /approvals/:id/allow-always`,
  `GET`/`POST /grants`, `DELETE /grants/:id`. **Dashboard:** Approve once vs Allow always on a gated
  tool approval, plus an Allow-always grants panel with revoke. Reference-first against openclaw
  (permission-relay allow-once|allow-always|deny, exec-host-gateway persist-only-when-safe,
  exec-approvals `hasDurableExecApproval` exact-match + `recordAllowlistUse`) and Hermes
  (`approval.py` frozen-trust). **Hermes-first Prime:** reference-read against Hermes
  (`reference/hermes-agent-main/agent/{prompt_builder,system_prompt,conversation_loop,
  chat_completion_helpers,message_sanitization}.py`), Prime is re-grounded as a general-purpose
  local AI agent / chat companion that drives the control plane only when the user asks for work.
  Greetings, small talk, venting, insults, emotional support, and general Q&A are first-class
  conversation, never work: two new wire-compatible `PrimeIntent` variants (`SmallTalk`,
  `EmotionalSupport`), a final conservative `classify_intent` pass that routes chitchat → `SmallTalk`
  and venting → `EmotionalSupport` only after every action/status/question/greeting rail (explicit
  work and real questions always win first), contextual non-action chips in place of bare CTA
  suppression, and general-agent prompt identity across the brain and sub-prompts; `is_chat_guarded`
  is strengthened so a brain can never reconcile an insult or vent up to a work intent, and
  brainstorm work CTAs attach only when a real work verb is present. Built reference-first per
  `docs/reference-driven-development.md`; `cargo test` + `clippy` clean on `relux-core`/`relux-kernel`,
  dashboard tests + typecheck + build green, the tracked `dashboard-dist` bundle rebuilt and committed
  in sync. Every safety property from v0.1.20 still holds.
- **v0.1.20** (2026-06-11) — a **third token-authenticated manager-subtree action** slice on
  top of v0.1.19, driven by `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§22). After `grant_permission`
  (§19/§20) and `assign_task` (§21), a manager that authenticated its OWN request with a per-agent
  access token may now do a third self-driven action — revoke an explicit permission from one of
  its own-Branch subordinates — with no operator in the loop, and no master-plan safety property is
  weakened. **Token-authenticated `revoke_permission` (§22):** no new permission grammar
  (`agent:<id>:subtree:<action>` is action-generic, so `:revoke_permission` parses/stores/revokes
  unchanged and the pure `manager_subtree_authorizes` matcher takes the action with no cross-action
  bleed); `KernelState::manager_revoke_permission_from_subordinate` routes through the same
  own-Branch + Active-manager + exact-scope chokepoint, checked first, and removes EXACTLY the
  stored grant via the unchanged `revoke_permission_from_agent` (`matches_exact`, no pattern
  expansion), so an unheld permission is the honest `PermissionNotGranted` (404), an
  unauthorized/out-of-Branch/unknown target → 403, a malformed body → 400, every denial audited.
  **Agent-authenticated surface:** `POST /v1/relux/agents/me/manager-revoke` on the bearer
  `agent_router`, body `{target_id, permission}`, where the acting manager is the token subject
  (never the body), with an `agent:token_authenticated_manager_revoke_permission` provenance audit
  (public `token_ref` only); operator routes stay closed to bearer tokens. **Manager-actions UI:**
  the Crew Governance "Manager actions (token-authenticated)" panel now offers a local test form
  for all three agent-self routes (manager-grant / assign-task / manager-revoke), each a compact
  collapsible form requiring the operator to paste the copy-once raw token deliberately
  (`type=password`, cleared after), with a Branch target picker, exact-permission validation, a
  secret-free curl snippet (`$RELUX_AGENT_TOKEN`), and a bearer helper (`agentSelfManagerGrant` /
  `agentSelfAssignTask` / `agentSelfManagerRevoke`) sent with `credentials: omit` so the operator
  session never bypasses the token path. Built reference-first per
  `docs/reference-driven-development.md`; `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel`, dashboard tests + typecheck + build green, the tracked
  `dashboard-dist` bundle rebuilt and committed in sync. Every safety property from v0.1.19 still
  holds.
- **v0.1.19** (2026-06-11) — a **second token-authenticated manager-subtree action** slice on
  top of v0.1.18, driven by `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§21). Building on the §20
  per-agent identity, a manager that authenticated its OWN request with a per-agent access token
  may now do a second self-driven action — assign an existing task to one of its own-Branch
  subordinates — with no operator in the loop, and no master-plan safety property is weakened.
  **Token-authenticated `assign_task` (§21):** no new permission grammar
  (`agent:<id>:subtree:<action>` was already action-generic, so `:assign_task`
  parses/stores/revokes unchanged and the pure `manager_subtree_authorizes` matcher takes the
  action with no cross-action bleed); `KernelState::manager_assign_task_to_subordinate` routes
  through the same own-Branch + Active-manager + exact-scope chokepoint, checked first, keeps the
  single-pointer model (`assigned_agent` → `Queued`), and adds one assignability guard (terminal
  task → 409 `TaskNotAssignable`, missing task → `UnknownTask` 400, unauthorized/out-of-Branch/
  unknown target → 403), every denial audited. **Agent-authenticated surface:**
  `POST /v1/relux/agents/me/assign-task` on the bearer `agent_router` where the acting manager is
  the token subject (never the body), with an `agent:token_authenticated_manager_assign_task`
  provenance audit (public `token_ref` only); operator routes stay closed to bearer tokens.
  **Manager-actions UI:** a compact Crew Governance "Manager actions (token-authenticated)" panel
  documents both agent-self routes (manager-grant / assign-task) with the required scope, shows
  secret-free curl snippets (`$RELUX_AGENT_TOKEN`), and offers a local assign-task test form that
  requires the operator to paste the copy-once raw token deliberately (`type=password`, cleared
  after) and drives the `agentSelfAssignTask` bearer helper (`credentials: omit`) so the operator
  session never bypasses the token path. Built reference-first per
  `docs/reference-driven-development.md`; `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel`, dashboard tests + typecheck + build green, the tracked
  `dashboard-dist` bundle rebuilt and committed in sync. Every safety property from v0.1.18 still
  holds.
- **v0.1.18** (2026-06-11) — a **first per-agent identity** slice on top of v0.1.17, driven by
  `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§20). It closes — for the `grant_permission` action —
  the §19 operator-assisted gap: a manager can now authenticate its OWN request and drive the
  manager-subtree grant with no operator in the loop, and no master-plan safety property is
  weakened. **Bounded, hash-only agent token (§20):** a new `AgentTokenStore` mints an opaque
  `relux_agt_` token bound to one agent, stored only as its SHA-256 hash in a gitignored,
  atomic, permission-restricted file, with a bounded/clamped TTL and individual revocation; the
  raw token is shown once at mint, mapping Paperclip's `agent-auth-jwt.ts` (`sub`/`exp`,
  timing-safe) to a local hashed opaque token and reusing the `auth.rs` hashed-store discipline.
  **Agent-authenticated surface:** a `require_agent_token` bearer middleware gates a two-route
  allowlist (`GET /v1/relux/agents/me`, `POST /v1/relux/agents/me/manager-grant`) where the
  acting agent is always the token subject (never the body); there is no `RELUX_AUTH_DISABLED`
  bypass on the agent surface, an agent token is never accepted on an operator route, and
  operator-only mint/list/revoke lives under `/v1/relux/agents/:id/tokens`.
  **Token-authenticated manager-grant:** `manager_grant_permission_to_subordinate_as_agent`
  runs the unchanged own-Branch + Active + scope gate and audits token provenance (public id
  only), `redact` masks the `relux_agt_` prefix, and a Crew Governance "Access tokens" panel
  mints (copy-once), lists metadata, and revokes. Built reference-first per
  `docs/reference-driven-development.md`; `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel`, dashboard tests + typecheck + build green, the tracked
  `dashboard-dist` bundle rebuilt and committed in sync. Every safety property from v0.1.17
  still holds.
- **v0.1.17** (2026-06-11) — a **scoped-permission + chain-of-command governance** slice on
  top of v0.1.16, driven by `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§3/§5/§6/§16–§19). Every new
  path stays fail-closed, governed, and bounded; no new authority is granted that the holder
  did not already have, and no master-plan safety property is weakened. **Bounded conversation
  compaction (§6/§16):** Prime's conversation memory keeps a bounded, deterministic compaction
  of history older than the 12-turn working ring, summarizing rather than dropping older
  context — advisory prompt-context only, no provider call required. **Scoped tool-plugin
  grants (§5/§17):** a single strictly-validated `tool:<plugin-id>:*` permission authorizes
  every concrete tool in one plugin; `Permission::new` accepts the wildcard only in that exact
  shape (broad/partial globs and injection strings rejected fail-closed as `MalformedScope`),
  and enforcement moves grant-vs-required to a new `authorizes()` (exact OR same-plugin scope)
  while grant dedup/revoke stay exact-match so a scope never pattern-expands. **`reports_to`
  org-lattice (Lead) model (§3/§18):** `Agent` gains an optional `reports_to` and a pure
  `hierarchy` module (`chain_of_command` / `is_in_subtree` / `would_create_cycle`, bounded by
  `MAX_HIERARCHY_DEPTH = 50`, total even on a cyclic map); the kernel resolves a Lead, rejects
  reporting cycles under the lock, and enriches Crew cards — display + validation only, no
  permission widened. **Manager-subtree scoped grant (§5/§19):** a strict
  `agent:<manager-id>:subtree:<action>` grammar + pure `manager_subtree_authorizes` matcher
  (manager == holder AND action matches AND target a proper descendant; self/sibling/ancestor/
  unrelated denied), wired to ONE real enforcement path — an Active manager granting a
  permission to an operative inside its OWN Branch. **Operator-assisted HTTP/UI surface (§19):**
  a governed `POST /v1/relux/agents/:id/manager-grant` (behind `require_session`) + a Crew
  "Grant as manager" panel; honest trust boundary — this is **operator-assisted**, not
  per-agent-authenticated, so the authenticated operator stands in as the named, audited
  authorizer (`operator:authorize_manager_grant`) and cannot widen what the manager itself
  could do. Built reference-first per `docs/reference-driven-development.md`; `cargo test` +
  `clippy` clean on `relux-core`/`relux-kernel`, dashboard tests + typecheck + build green, the
  tracked `dashboard-dist` bundle rebuilt and committed in sync. Every safety property from
  v0.1.16 still holds.
- **v0.1.16** (2026-06-11) — an **agentic run recovery + durable session/handoff** slice on
  top of v0.1.15, driven by the new `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` (§1/§3/§7/§15). Every
  new path stays fail-closed, governed, and bounded; no new authority is granted and no
  master-plan safety property is weakened. **Durable run session / safe Claude resume
  (§3/§15):** a CLI adapter's reported provider session id (the Claude `--output-format json`
  `session_id`) is captured as a bounded, redacted `RunSession` on the `Run`
  (`sanitize_session_id` rejects argv injection; `plan_resume` is the single source of truth),
  and a governed `POST /v1/relux/runs/:id/resume` continues that session for the one adapter
  that supports safe non-interactive resume (Claude `-p --resume <id>`), refusing honestly
  elsewhere (`RunResumeNotSupported`, 422); re-run stays a distinct lineage. The dashboard Work
  run-detail gains a copyable Session row, an honest Handoff label, and a Resume button (maps
  OpenClaw `acp-spawn` `resumeSessionId` / `runCliWithSession`). **Structured failure +
  bounded transient retry (§7):** `run_failure.rs` adds a priority-ordered `RunFailureClass`
  with `retryable` / `needs_operator_action` / `remediation`, a bounded backoff schedule
  (`[2m, 10m, 30m, 2h]`), and a redact+clamp `safe_public_message`; only
  `transient_provider`/`timeout` auto-retry (`unknown` stays manual since a run can mutate a
  workspace), with **no background scheduler** — `not_before` is real wall-clock consumed
  manually or on an autonomy tick through the unchanged governed `retry_run` path. Doctor gains
  a `runs.recovery` row; the Work page shows a failure-class chip + remediation (grounded in
  Hermes `error_classifier.py`, Paperclip `run-liveness.ts` / `heartbeat.ts`). **Bounded Prime
  self-correction (§1/§7):** the observe-then-act `DecisionLoop` re-asks a malformed-but-
  correctable brain reply **once** (`MAX_DECISION_CORRECTIONS=1`) with `parse_decision`'s own
  error injected, instead of silently falling back; a corrected decision grants no authority
  and still flows through the unchanged fail-closed gate, and the legacy
  `step` / `run_decision_loop` are preserved byte-for-byte (mirrors Hermes
  `_invalid_json_retries` / openclaw retry instructions). Built reference-first per
  `docs/reference-driven-development.md`; `cargo test` + `clippy` clean on
  `relux-core`/`relux-kernel`, dashboard tests + typecheck + build green, the tracked
  `dashboard-dist` bundle rebuilt and committed in sync. Every safety property from v0.1.15
  still holds.
- **v0.1.15** (2026-06-11) — a **cross-platform source launcher + read-only kernel Doctor**
  slice on top of v0.1.14. Both surfaces are read-only / launch-only and leak no paths or
  secrets; no master-plan safety property is weakened. **Cross-platform launcher:** a Bash
  `start-relux.sh` counterpart to `Start-Relux.ps1` for macOS/Linux source checkouts —
  locates the repo root, checks `cargo` (rustup guidance if missing), builds/reuses
  `target/debug/relux-kernel` (`--release` optional, `RELUX_CARGO_JOBS` cap), sets the same
  `RELUX_HTTP_ADDR` / `RELUX_DB` / `RELUX_DASHBOARD_DIST` env vars, preflights the loopback
  port (`nc`, falling back to bash `/dev/tcp`) with an actionable busy-port error, and runs
  `serve` in the foreground; flags `--port` / `--release` / `--dry-run` / `--doctor` /
  `--help`. The README separates the three launch paths (prebuilt Windows zip; Windows
  source; macOS/Linux source) and is explicit that the packaged zip is Windows-x64 only;
  `.gitattributes` pins `*.sh` to LF. **Read-only Doctor (`relix-dashboard-design.md`
  §15.1):** a session-protected `GET /v1/relux/doctor` emits a structured diagnostics report
  reusing the same cheap reads as `/v1/relux/health` (store, dashboard bundle, AI status,
  adapter + tool readiness, agent + approval counts) as `ok`/`info`/`warn`/`fail` rows with
  message, remediation, and an in-app action link — no heavy work, no mutation, and no
  paths/secrets (`DoctorInputs` carries no filesystem path; severity rules mirror
  `readiness.ts`). The dashboard gains a compact worst-first Doctor panel on Health with Fix
  links, a Refresh, and an honest error state; pure helpers in `doctor.ts`. Built
  reference-first per `docs/reference-driven-development.md` (Hermes `doctor.py`
  check_*/_fail_and_issue; openclaw `health-state.ts` includeSensitive). Proven by
  `doctor.rs` unit tests (every severity rule + redaction), a server test pinning
  session-gating / the row set / no db-path leak, frontend `doctor.test.ts`, and
  `doctor-render.test.mjs`; the tracked `dashboard-dist` bundle was rebuilt and committed in
  sync. Every safety property from v0.1.14 still holds.
- **v0.1.14** (2026-06-11) — a **manual Crew configuration + permissions governance** slice
  on top of v0.1.13 (`relix-dashboard-design.md` §9 / §9.1). Every new surface is
  operator-driven and fails closed; no master-plan safety property is weakened, and
  `create_agent` still grants only the minimal echo tool. **Manual create/edit:** a shared
  Crew create/edit form (name, id, role, persona, adapter/runtime, status) backed by
  `agent_config.rs` (pure, unit-tested validation/sanitization — strict id, id+name
  uniqueness, adapter-must-resolve, status allowlist, persona bounded + secret-redacted),
  field-granular audited `KernelState::update_agent`, persona-accepting `POST
  /v1/relux/agents`, and a new `PATCH|PUT /v1/relux/agents/:id`; honest `400`/`404`.
  **Explicit-permission view + safe revoke:** Crew cards list explicit permissions (elevated
  control-plane grants flagged), `revoke_permission_from_agent` audits + fails closed
  (`PermissionNotGranted` → `404`) via `DELETE /v1/relux/agents/:id/permissions`, and a pure
  `governance.ts` mirrors `VALID_PREFIXES` to gate elevated grants behind a deliberate
  confirm; Prime's own `GrantPermission` stays approval-gated. **Skills/tags + skill-aware
  assignment:** a bounded specialty-tag list on `relux_core::Agent` (serde-default,
  snapshot-compatible) drives Prime fuzzy assignee resolution (sole holder routes,
  shared-skill is ambiguous, exact id/name wins); skills are specialty for routing only,
  never a capability gate. **Safe role presets:** read-only `GET /v1/relux/agent-presets`
  (pure, unit-tested `agent_presets.rs`) seeds create-form role/persona/skills; `POST
  /v1/relux/agents` accepts an optional preset id that fills only omitted fields and flows
  through the same validators (unknown preset → `400`); the `AgentPreset` type has no
  permission/adapter field, so it cannot widen power. Built reference-first per
  `docs/reference-driven-development.md` (openclaw sessions-spawn-tool / approval-classifier
  / tool-policy, Hermes system_prompt + message_sanitization). Proven by new
  `agent_config` / `agent_presets` / `governance` unit tests, extended
  `agent_create_and_edit_workflow_over_http` and
  `agent_presets_list_and_create_with_preset_over_http` kernel tests, dashboard
  `governance.test.ts` / `presets.test.ts`, and the `crew-render` harness; the tracked
  `dashboard-dist` bundle was rebuilt and committed in sync. Every safety property from
  v0.1.13 still holds.
- **v0.1.13** (2026-06-11) — an **in-app first-run / operational readiness guide +
  dashboard build hygiene** slice on top of v0.1.12. Entirely dashboard-side: no new
  product surface, no new endpoint, and no master-plan safety property weakened.
  **Readiness guide:** a pure `apps/dashboard` `buildReadiness()` (`readiness.ts`) turns
  the four control-plane reads Home already makes (state, `ai/status`, adapters,
  plugins+tools) into one honest report — items for Prime brain (reusing
  `onboarding::primeBrainStep`), real-work adapter, crew (local-Prime fallback),
  plugins/tools (reusing `plugins::pluginCategory`/`toolReadiness`), and pending
  approvals. A *selected-but-broken* brain is the only blocker; a local brain works;
  metadata wrappers / unconfigured runtimes are surfaced as attention, never a faked
  green check. `ReadinessGuide.tsx` renders one compact card (setup = checklist with
  per-item action links; operational = concise summary + single first action behind
  `<details>`), shared across Home (replacing its static checklist; redundant
  Claude/Codex prose card dropped) and Health (same derivation over the reads it already
  makes — no duplicated logic). **Partial-read honesty:** a `ReadinessFailed` flag set
  distinguishes a failed read from one still in flight — a failed read becomes a
  retryable "… unavailable" warn row and forces the report degraded (`ready` = false) so
  a Health-OK-but-state-read-failed instance never paints a faked "operational" badge,
  while a still-loading read stays a neutral "Checking readiness…" row. **Build
  hygiene:** the dashboard `typecheck` script type-checks each project directly so
  `npm run typecheck` passes (was failing TS6310 on composite `tsconfig.node.json`), and
  route-level `React.lazy` + a `manualChunks` vendor rule replace the old single ~653 kB
  bundle (largest chunk now the ~165 kB vendor chunk) so `vite build` no longer warns
  about chunks over 500 kB — same components at the same paths, per-route chunks behind a
  Suspense fallback. Built reference-first per `docs/reference-driven-development.md`
  (in-app readiness guide) and conforms to `docs/relix-dashboard-design.md` §15. Proven
  by `readiness.test.ts` plus the `readiness-render` / `health-render` /
  `readiness-guide-render` `.mjs` render harnesses; the tracked `dashboard-dist` bundle
  was rebuilt and committed in sync. Every safety property from v0.1.12 still holds.
- **v0.1.12** (2026-06-11) — a **source-checkout launcher + bounded Prime conversation
  memory** slice on top of v0.1.11. No new product surface and no master-plan safety
  property weakened; this line makes the documented one-command boot actually work from a
  cloned repo and gives Prime's brain a small, fenced sense of recent context.
  **Root source launcher:** a repo-root `Start-Relux.ps1` (separate from the prebuilt
  bundle launcher of the same name) locates the workspace root via `$PSScriptRoot` with a
  guard, builds or reuses `target\{debug,release}\relux-kernel.exe` (cold builds capped via
  `scripts\cargo-jobs.ps1`), points the kernel at the committed `dashboard-dist` and the
  gitignored `dev-data\` store, runs the same loopback port preflight as the bundle
  launcher, prints the dashboard URL, and serves in the foreground; flags `-Port`,
  `-Release`, `-DryRun`, `-Doctor`, `-Help`. **Bounded conversation memory:** a small,
  bounded, secret-redacted per-conversation turn history (`relux_core::ConversationTurn`;
  `relux-kernel/prime_history.rs` with `MAX_HISTORY_TURNS=12`,
  `MAX_HISTORY_CONVERSATIONS=32`, `MAX_CONTEXT_CHARS=2000`) lets the brain interpret
  follow-ups ("what about the second one?", "do that again") in context. It is persisted via
  the meta-snapshot seam, injected into `build_decision_prompt` as a labelled BACKGROUND
  block BEFORE the current message (empty history leaves the prompt byte-for-byte
  unchanged), and recorded AFTER the reply is shaped so the stored reply is the FINAL
  user-visible one, with read-only context summaries surfaced as a "(consulted: …)" sub-line
  (never raw tool JSON / provider envelopes). The history is **advisory prompt context with
  zero authority** — it never reaches `classify_intent`, the fail-closed `reconcile_intent`
  gate, or any existence/approval check (those run on the current message alone), so it can
  never promote chat into work or override an explicit current-turn intent; a new
  `POST /v1/relux/prime/reset` (and an in-UI Clear button) wipes only this advisory memory.
  Built reference-first per `docs/reference-driven-development.md` (Hermes
  `run_conversation` history threading + `build_memory_context_block` fence + redact;
  openclaw hook-history slice + `buildCliSessionHistoryPrompt` + transcript-redact). Proven
  by `relux-kernel` / `relux-core` unit + integration tests (including
  `recorded_reply_is_the_final_shaped_reply_not_the_grounded_one`); every safety property
  from v0.1.11 still holds.
- **v0.1.11** (2026-06-11) — a **plugin tool-invocation** slice on top of v0.1.10. Where
  v0.1.10 closed the Prime observe-then-act + governed-orchestration line, this line makes
  the ToolSet-plugin tool-invocation surface honest and usable end-to-end on the
  dashboard, with no safety property weakened. **In-UI tool configuration:** a fail-closed
  `plugin_tool_config` parser (allowlisted fields, sanitize/clamp, `RiskLevel` allowlist)
  plus `KernelState::configure_plugin_tool` / `remove_plugin_tool` add or replace one tool
  on an installed, non-bundled ToolSet manifest, transactionally on a re-validated clone,
  with the permission DERIVED (`tool:<id>:<verb>`, never operator-supplied), via
  `POST`/`DELETE /v1/relux/plugins/:id/tools` and an in-UI add-a-tool form. **Honesty
  fix:** the manifest `approval` field is now load-bearing via
  `relux_core::approval_blocks_direct_invocation` behind a new
  `ToolExecutability::NeedsApproval` refusal in `call_tool`/`invoke_tool`, so a
  non-low-risk configured tool is never runnable just because a loopback runtime is enabled
  (bundled fixtures are `approval:never`, unchanged). **Honest readiness classifier:** a
  single `toolReadiness` helper (mirroring openclaw `approval-classifier`) maps the
  kernel's six executable states to `{ runnable, label, tone, reason, nextStep }`
  (`runnable` true only for `ready`); every non-ready tool renders an inline "Why not?"
  panel with the refusal/disabled reason + next step, never a blank page. **Per-tool-call
  approval flow:** an operator requests approval for ONE invocation (tool id + exact args)
  via `request_tool_invocation_approval` (`POST /v1/relux/tools/request-approval`),
  creating a Pending Approval + a `PendingToolInvocation` binding to the exact
  `(plugin, tool, agent, args + SHA-256)`; `execute_approved_tool_invocation`
  (`POST /v1/relux/approvals/:id/execute`) runs only when Approved AND unconsumed,
  re-validates existence/permission/args-hash, executes the STORED snapshot (never
  client-resupplied args), and consumes the binding on a single attempt. Built
  reference-first per `docs/reference-driven-development.md` (openclaw two-phase exec
  approval + consume-once handoff + approval-classifier). No blanket/reusable grant; no
  remote/non-loopback execution; `decide → prime_execute / approval` stays the sole
  durable-state path. Proven by `relux-kernel` / `relux-core` unit and integration tests
  plus dashboard `toolReadiness` assertions; every safety property from v0.1.10 still
  holds.
- **v0.1.10** (2026-06-11) — a **Prime observe-then-act + governed orchestration** slice
  on top of v0.1.9. Where v0.1.9 gave the brain a single-shot governed tool surface, this
  line lets one turn *inspect then act* and extends the safe write surface to
  orchestration, while every safety property holds and the brain changes no state
  directly. **Bounded observe-then-act loop:** the unified `PrimeBrainDecision` call now
  loops (`DecisionLoop` / `MAX_DECISION_ROUNDS`) — each round the brain may request
  read-only context tools (run deterministically against the pre-taken snapshot and
  re-asked, grounded in the results) or commit one decision, so one turn can inspect live
  state → choose one governed action grounded in what it saw → execute/propose → narrate;
  the observe phase has no mutation path, the action still flows through the unchanged
  fail-closed `reconcile_intent` gate + `decide → prime_execute` / approval, and the loop
  is bounded, stops on no-progress, and yields an interim decision on failure (round one's
  prompt is byte-for-byte the prior single-shot). **Governed `orchestration.create`:**
  maps to the existing deterministic `plan_orchestration → prime_orchestrate`
  (OrchestrateGoal) path — the brain proposes only the goal text (advisory step hints);
  the deterministic planner keeps full authority over briefs, role classification,
  live-roster agent grounding, the step cap, the dependency DAG, and the multi-agent gate,
  and the sensitive-intent gate keeps guarded chat from ever triggering a create.
  **Governed `orchestration.start`:** a new `PrimeIntent::OrchestrationRun` /
  `PrimeAction::RunOrchestration` runs an existing governed batch — `prime_execute`
  validates the `orch_` id against live records (unknown → honest reply, fail closed) then
  runs the existing `run_orchestration` batch (max 25, concurrency 2), with multi-turn
  clarify memory ("run the orchestration" → "which one?" → "orch_0001") and a
  deterministic run reply. **Dashboard:** the Plugins page now shows live adapter runtime
  state inline (same `GET /v1/relux/adapters` probe as Crew: `local_deterministic` /
  `available` / `missing_binary` / `disabled` / `needs_configuration`, fail-closed to an
  honest "status unavailable"), and protected Claude/Codex adapter rows now expose a real
  "Configure" path to `/crew` instead of a dead-end "locked" (protected = locked against
  removal only). Built reference-first per `docs/reference-driven-development.md` (Hermes
  + Paperclip/openclaw) and audited in `docs/prime-processing-audit.md`. Proven by
  `relux-kernel` / `relux-core` unit and integration tests; every safety property from
  v0.1.9 still holds.
- **v0.1.9** (2026-06-11) — a **Prime tool-use loop** slice on top of v0.1.8. Where
  v0.1.8 made Prime brain-mediated for intent/slots/wording, this line gives the brain a
  *governed tool surface* — first to read live control-plane state, then to request a
  single mutating action — while every safety property holds and the brain changes no
  state directly. **Safe read-only context loop:** Prime inspects live state through a
  fail-closed, bounded allowlist (`get_run`, `list_plugins`, `list_approvals`, and the
  state views) before answering; the brain proposes tool names, the allowlist gate drops
  any mutating/unknown name at parse time, the loop is capped by `MAX_TOOL_ROUNDS`, and
  the reply is grounded only in redacted observations — no raw provider envelope, no path
  to `prime_execute`. These reads now also ride the **unified decision envelope** and are
  validated through the same allowlist, with **dashboard provenance** (a `used: <tool>`
  chip plus a bounded, collapsed per-read detail). **First safe WRITE-capable tool
  surface:** a brain may request ONE governed mutating tool per turn (`task.create`,
  `task.update`, `task.assign`, `task.start`, `agent.create` as safe Acts; `plugin.install`
  and `permission.grant` as approval-gated Proposes), which Relux desugars into an existing
  Prime action/proposal and routes through every current slot/intent/approval gate — the
  fail-closed intent gate still vetoes a mutating tool on guarded chat, every id is
  validated against live state, batched mutating requests are refused, and
  `decide → prime_execute / approval` stays the sole path that changes durable state.
  **Safe post-execution after-action narration:** after the kernel has already executed
  (or proposed) an action through the unchanged path, a brain may re-word the final
  confirmation grounded only in a sanitized, bounded result envelope and validated against
  it (completion claims honored only when the fact is confirmed; success-on-failure,
  installed/granted-on-proposal, and invented ids rejected; secrets/paths redacted),
  changing no state. Built reference-first per `docs/reference-driven-development.md`
  (Hermes + Paperclip/openclaw + open-webui) and audited in
  `docs/prime-processing-audit.md`. Proven by `relux-kernel` / `relux-core` unit and
  integration tests; every safety property from v0.1.8 still holds.
- **v0.1.8** (2026-06-11) — a **Prime intelligence** slice on top of v0.1.7 that makes
  Prime brain-mediated end to end while keeping every safety property. The deterministic
  keyword cascade is now only the **fallback rail**: a configured brain (OpenRouter or
  the local Claude/Codex CLI) genuinely decides each turn, and every brain output is
  validated against the live state behind a **fail-closed gate** before anything mutates.
  **Brain-mediated intent:** the brain proposes a `PrimeIntent`, validated against the
  allowlist and reconciled by a gate that may sharpen but never weaken a misread.
  **Brain-assisted validated slots:** task creation (title/details/assignee/priority),
  agent hiring, plugin install, and permission grants are filled by the brain and
  hard-validated, with brain-refined clarification wording and a persona seed.
  **Multi-turn clarify memory:** a follow-up answer continues the prior clarify instead
  of restarting. **Fuzzy continuation:** roster-aware assignee resolution and
  brain-assisted assignment continuation, plus **by-id run start** with a resolvable
  run-start clarification. **Safe by-id task UPDATE:** a real mutating action with an
  allowlisted field set, clamped/sanitized values, a terminal-state guard, and no fake
  completions. **Unified decision envelope:** one brain call now carries intent + slots
  + clarification wording + conversational reply + plan-preview polish, computed off-lock
  and validated post-turn through the existing chokepoints (`validate_polish`,
  `parse_adapter_result`, the slot/intent gates), so a single round trip drives the whole
  turn without loosening any guard. Built reference-first per
  `docs/reference-driven-development.md` (Hermes + Paperclip/openclaw) and audited in
  `docs/prime-processing-audit.md`. Proven by `relux-kernel` / `relux-core` unit tests;
  every safety property from v0.1.7 still holds.
- **v0.1.7** (2026-06-11) — a product slice on top of v0.1.6 that adds a first-class
  **idea → plan → tasks** rung and hardens the dashboard against page crashes.
  **Plan-preview proposal card:** Prime renders an *action-free* preview of a
  decomposed plan (goal heading, an *N steps across M agents* summary, and each
  proposed step with its role + assignee); the card commits nothing — an explicit
  one-click **Create these tasks** (multi-step) / **Turn this into a task**
  (single-step) is the lone commit path, keyed off the same decomposition the card
  shows. **Advisory plan polish:** an optional LLM pass may refine only the *wording*
  of that card (summary, per-step titles, clarifying questions, risk notes) while the
  deterministic planner stays the sole authority on step count/order/agent
  grounding/goal/commit; it runs through one `validate_polish` chokepoint on both the
  OpenRouter brain and the local Claude/Codex CLI brains, and the card shows **which**
  brain refined the wording (OpenRouter model id or CLI brain label). **Conversation
  guard:** questions and musing now stay chat and never silently mint work even when
  the sentence carries an action verb, while an explicit command still overrides and
  mints/runs work. **Route-level `ErrorBoundary`:** every routed page mounts inside an
  error boundary, so a render-time throw in one view degrades to an in-app error card
  with Reload/Retry instead of blanking the whole SPA; a `work-render` SSR test pins
  **Work** mounting under the plain declarative `<BrowserRouter>` the app uses. Also
  folds in the **blank-Crew-page fix** (Crew loaded via `useLoaderData()` under a
  declarative router and white-screened; now loads via `useAsync` with honest
  loading/error/empty/list states, rail entry repointed to `/crew`) and
  **reflect-and-clarify** for the Brainstorming, Orchestration single-step, and
  TaskUpdate clarify arms. Proven by `relux-kernel` unit tests (conversation guard,
  proposal wire shape, polish validation, clarify reflection) and dashboard tests
  (error-boundary helper, Work SSR render, proposal-card + polish-provenance
  helpers); dashboard bundle rebuilt. Every safety property from v0.1.6 still holds.
- **v0.1.6** (2026-06-10) — a user-facing patch on top of v0.1.5 that keeps **Prime
  conversational on ideation** and ships the post-v0.1.5 operator-session work that
  had not yet been bundled. **Prime stays conversational / deep-links / chat-first:**
  brainstorming no longer auto-creates tasks — `classify_intent` treats musing
  lead-ins ("I was thinking…", "what if we…", "I have an idea…") as **Brainstorming**
  even when the sentence carries a creation verb, so *"I was thinking to create an
  n8n-like program using 20 agents"* stays a conversation, while an **explicit
  command** (`create a task to…`, `orchestrate`, `assign`, `start it`) still
  overrides and mints/runs work; Prime task/run links now deep-link into the Work
  surface via `/work?task=<id>` (and `/work?run=<id>`), opening that item's detail
  panel focused and degrading honestly when the id is missing; and the Prime page is
  **chat-first**, with Autonomy + Orchestration moved into a collapsed **Advanced**
  disclosure below the input and an honest hint that brainstorming stays a
  conversation. **Restart-persistent operator sessions (auth v1.2):** the v0.1.5
  caveat that sessions were in-memory (a `serve` restart forced everyone to
  re-sign-in) is closed — the session table is mirrored to a gitignored local file
  (`dev-data/relux/dashboard-sessions.json`; `RELUX_SESSION_FILE` overrides) next to
  the admin credential, with the same atomic, OS-permission-restricted write as
  `dashboard-admin.json`. What persists is a **SHA-256 hash of each opaque session
  id** plus its metadata (username, idle deadline, absolute deadline) — **never the
  raw id**, so a leaked file cannot be replayed as a cookie; expired rows are pruned
  on load and use; logout removes the row, a password change invalidates every
  *other* session on disk (keeping the caller's), and `reset-admin` now also **clears
  the session file**. **Live session-file reconcile / no-restart revocation (auth
  v1.3):** a **running** `serve` now picks up an out-of-band session-file change
  without a restart — before every session operation the store cheaply re-`stat`s its
  backing file (fingerprint = mtime + length, plus a "file absent" state) and only
  when that differs reconciles its in-memory table with disk: a **deleted** file
  (what `reset-admin` does) drops all in-memory sessions (fail-closed), and an
  external **rewrite** is adopted instead of clobbered; `create`/`refresh` reconcile
  *before* they persist, so a fresh login after a delete cannot rewrite revoked
  sessions back. **Effect:** `reset-admin` invalidates old cookies on a running
  server on the **next request** — no restart required. **Absolute session cap ruled
  intentional (auth v1.4):** the hard **absolute** ceiling
  (`SESSION_ABSOLUTE_MAX_SECS`) is wall-clock from session mint and is **never**
  extended by activity — only a fresh re-auth (logout + new login) re-anchors a new
  window; the `auth.rs` doc comment now states this and a lib test
  (`a_fresh_login_re_anchors_the_absolute_window_but_activity_never_does`) pins both
  halves (no behavior change). Proven by `relux-kernel` unit + in-process HTTP tests
  (restart survives, no raw sid on disk, external delete revokes on a live handle,
  delete + new login doesn't resurrect, the absolute-window decision) and dashboard
  routing tests (the `?task=`/`?run=` deep links). *Caveats:* one admin only (no
  multi-user, roles, or per-operator audit); the loopback API has **no transport
  TLS**; reconcile detection is `stat`-granularity (revocation bites on the next
  session-touching request, and a same-mtime-and-same-length external *rewrite* could
  be missed, though *deletion* — the recovery case — is always caught); and
  `RELUX_AUTH_DISABLED` leaves the surface fully open by design. Every safety
  property from v0.1.5 still holds on every path.
- **v0.1.5** (2026-06-10) — first build on top of v0.1.4 that puts a **single-admin
  local operator login** in front of the standalone dashboard/API; the surface is no
  longer open by default. **First-run admin setup + login:** on first launch the
  dashboard shows a one-time setup screen to set the local admin password; thereafter
  a sign-in screen gates the dashboard and the `/v1/relux/*` API, with the session
  carried in an HTTP-only `relux_session` cookie. The admin credential is stored next
  to the DB Argon2-hashed (never the plaintext); `relux-kernel reset-admin [user]
  [pw]` is the recovery path when the current password is unknown, and
  `RELUX_AUTH_DISABLED=1` is a documented dev/test bypass that `serve` warns about
  loudly. **Password change in-console:** the dashboard **Account** panel changes the
  admin password (verifies the current one, enforces the length floor) without
  disturbing the live session. **Sliding session refresh:** an authenticated request
  slides the session forward up to a hard **absolute** ceiling; sitting idle past the
  rolling window signs the operator out. The public, **non-sliding** `GET /v1/auth/me`
  returns safe, secret-free session metadata — the idle and absolute deadlines plus
  seconds remaining, the configured policy windows, and the server clock — and never
  exposes the session id, cookie value, or admin hash (a test asserts the body
  contains neither). **Account session readout + expiry warning + one-click re-auth:**
  the Account panel shows the idle/absolute policy with live countdowns; the shell
  topbar shows a quiet expiry chip (amber for the rolling idle window, red for the
  absolute ceiling) that opens Account; and Account offers a *"Sign out and sign back
  in"* re-auth action — promoted to the primary action with an alert banner inside the
  absolute warning window — that ends the session via `POST /v1/auth/logout` and
  re-shows sign-in. It **never** auto-submits credentials and never weakens auth, and
  re-auth mints a fresh session that resets the absolute window while invalidating the
  old cookie server-side. Proven by `relux-kernel` unit + in-process HTTP tests
  (setup/login/logout, sliding refresh, old-cookie server-side invalidation on
  re-auth, the `/v1/auth/me` no-secret contract), dashboard decision-helper tests
  (`sessionWarning` / `reauthCallout` / the local countdown basis), render/static
  proofs of the chip + Account re-auth promotion, and the standalone
  `scripts\relux-e2e-smoke.ps1` full E2E over HTTP against the real release binary.
  *Caveats:* one admin only (no multi-user, roles, or per-operator audit); sessions
  are **in-memory** and do not survive a `serve` restart (everyone re-signs-in); the
  loopback API has **no transport TLS**; the absolute ceiling can only be cleared by a
  fresh sign-in (no console action extends it); and `RELUX_AUTH_DISABLED` leaves the
  surface fully open by design. Every safety property from v0.1.4 still holds on every
  path.
- **v0.1.4** (2026-06-10) — first build on top of v0.1.3 that makes the
  orchestrator's **run results reviewable and applyable** and its **live progress
  honest**, while fixing a user-facing Prime-chat regression. **Prime CLI brain
  raw-JSON fix:** the Claude/Codex conversational path used the captured CLI stdout
  verbatim, so with Claude's `--output-format json` the chat bubble showed the
  whole result envelope (`type`, `result`, `is_error`, `usage`, `duration_ms`,
  `session_id`, `total_cost_usd`, …) instead of the human answer. The reply is now
  shaped through the same `parse_adapter_result` the assigned-run path uses — it
  lifts the envelope `result` text, degrades to plain prose for Codex/text mode,
  surfaces an `is_error` envelope as an honest fallback note (never the raw JSON),
  and falls back on an empty answer. Extracted as a pure, unit-tested seam
  (`shape_cli_brain_reply`) so the no-raw-JSON contract is pinned by tests.
  **First real Relux diff/apply model:** a run can capture **proposed changes** —
  read-only **artifacts** (name / type / summary / source, sanitized path + size)
  promoted into reviewed, applyable changes that **replace, create, rename/move, or
  delete** files, applied as a **single multi-file transactional apply** (all-or-
  nothing: a per-change precondition/traversal failure rolls the whole batch back,
  no partial writes), with the Prime conversational brain handling the
  `proposed_changes` envelope honestly. **Live-tail + stalled signals:** both the
  Relux **Work** Run Detail and the legacy **Run transcript** now do an efficient
  **incremental live-tail** (append only new transcript lines, not a full re-fetch)
  and show an honest **stalled / "No activity for Xs"** cue as a restrained
  badge-chip when an in-flight run goes quiet — consistent wording across both
  surfaces. **Orchestration cancel / resume / restart-honest:** orchestration jobs
  gained **cooperative cancel/stop** (a live, multi-brief in-flight job stops at the
  next safe point), **resume-after-cancel**, and **restart-honest** job status —
  after a server restart a poll by orchestration id reconstructs status from the
  durable record (`completed` / `interrupted`) and the dashboard shows an
  interrupted-job callout with a **Continue** resume. **Run Detail deep links + UX
  polish:** URL-driven in-shell Run Detail with orchestration `run_id` deep links, a
  **Copy link** action, consolidated in-shell run navigation on the Work surface,
  honest review/apply parity, per-brief recorded run duration, and a **status badge
  that carries the error tone** for failed runs (no longer the neutral chip). Also
  hardens first-run: an actionable **port-conflict** message on `serve` bind failure
  and a matching bundle-launcher port preflight, with their wording pinned to parity.
  Every safety property from v0.1.3 still holds on every path: dependency gating,
  at-most-once per round, permission + adapter-runtime gating before any spawn,
  secret redaction, the durable run transcript, audit, retry, sibling failure/panic
  isolation, and **no auto-run of downloaded plugin code**. Proven against the real
  Claude and Codex CLIs and by deterministic unit/HTTP smokes. *Caveats:* the
  transactional apply is the **Relux kernel** proposed-change surface (separate from
  the legacy `relix-runtime` brief-runs apply); the in-memory job registry still
  does not survive a restart for **by-job-id** polls (the by-orchestration-id poll
  stays restart-honest); live-tail is incremental polling, not a server-push event
  stream; retry/resume is a fresh attempt or a continued batch, not a partial-CLI-run
  resume; and the standalone API remains loopback-only (now gated by the
  single-admin local operator login added after v0.1.4).
- **v0.1.3** (2026-06-10) — first build on top of v0.1.2 that turns Prime from a
  single local task runner into a governed **multi-agent orchestrator**.
  **Multi-agent orchestration:** Prime decomposes a goal into role-typed briefs
  assigned to different agents and runs them as a governed batch
  (goal → brief → agent → run), instead of running one task itself.
  **Dependency-aware, round-based execution:** the planner infers simple ordering
  (implementation waits on research; testing/review/documentation wait on
  implementation) recorded as `depends_on` indices that only point at earlier
  briefs (a DAG by construction), and a round scheduler runs the ready set,
  repeats until nothing is ready or the round budget (1..=25) is spent, and
  honestly marks any brief whose dependency failed/blocked as **blocked** (never
  run, never faked). **Non-blocking, pollable jobs:**
  `POST …/orchestrations/:id/run-async` starts a background job and returns a job
  id immediately; `GET …/orchestration-jobs/:job_id` polls
  queued → running → completed/failed with the current round, per-brief statuses,
  running tallies, and the final aggregate (the worker persists the durable record
  between rounds, so a mid-batch poll sees real progress). **True bounded
  OS-parallel round execution:** independent briefs ready in the same round run as
  **real concurrent OS adapter processes** (one OS thread per brief, up to a
  concurrency cap, default 2, clamp 1..=4) with the kernel lock released around the
  spawn window — not one-at-a-time under the lock. **Sync API / CLI parallel
  parity:** the synchronous `POST …/orchestrations/:id/run` and
  `prime orchestration run --concurrency N` now drive the **same** shared parallel
  executor as the job worker (`prepare_orchestration_round` →
  `run_briefs_in_parallel` → `finalize_prepared_brief`), so there is one execution
  implementation, not two, and the paths can no longer diverge. Every safety
  property holds on every path: dependency gating, at-most-once per round,
  permission + adapter-runtime gating before any spawn, secret redaction, the
  durable run transcript, audit, retry, sibling failure/panic isolation, and **no
  auto-run of downloaded plugin code**. Proven by deterministic rendezvous tests
  (two slow fake adapters that complete only if running at the same instant) and
  against the **real Claude CLI**. *Caveats:* the in-memory job registry does not
  survive a server restart (a mid-job poll 404s; the dashboard falls back to the
  durable orchestration record); the concurrency cap is 1..=4 and the per-call
  round budget is 1..=25; dependency inference is conservative
  role-co-occurrence, not a full task graph; planning does not auto-create agents;
  no background timer drives orchestrations (operator-triggered only); and a retry
  is a fresh attempt, not a partial-run resume.
- **v0.1.2** (2026-06-10) — first build on top of v0.1.1 that closes the three
  honest post-v0.1.1 gaps (see *Status after v0.1.1*). **First-run onboarding:**
  Home's first-run checklist now derives a **live "connect Prime to a brain"
  step** from the control plane (`/v1/relux/ai/status` + `/v1/relux/adapters`) —
  it detects whether the Claude/Codex CLI is on PATH, reports whether the selected
  brain is actually usable, and routes the operator to Health → *Prime Brain / AI
  Runtime* with the exact next step. **Honest plugin install UX:** a generated
  metadata-only GitHub/zip wrapper is badged **Needs configuration** (never
  "enabled"/"ready"), its honest next step is **add tool definitions** (a one-click
  *Set up* with a copy/download manifest template), the install flow shows a
  **result summary** (tools discovered vs wrapper generated vs adapter), and the
  Tools list shows **only runnable tools** by default. **Adapter run depth:** a CLI
  adapter run is now observable and recoverable — Run Detail shows the adapter,
  status, phase, a real measured duration, a redacted output excerpt, a clear
  failure reason, and (when reported) cost/usage, all from the durable transcript;
  the Claude adapter requests a JSON result envelope parsed into an honest
  summary + metrics (`relux_core::parse_adapter_result`, `is_error` is a failure
  even on clean exit), Codex/generic degrade honestly to plain text, and a **failed
  run is retryable** as a fresh run with lineage (`retried_from`). Proven against
  the real Claude and Codex CLIs. *Caveats:* runs are synchronous (the page
  polls/refreshes rather than tailing live events), Codex/generic output is plain
  text (no structured envelope), and retry is a fresh attempt — not a resume of a
  partial CLI run.
- **v0.1.1** (2026-06-10) — first build that makes **Prime brain selection** a
  first-class dashboard surface. Health → *Prime Brain / AI Runtime* lets the
  operator pick who answers Prime's conversational turns — Local (deterministic),
  Claude CLI, Codex CLI, or OpenRouter — with one-click *"Use Claude/Codex for
  Prime"* that enables the adapter and selects the brain together. Live adapter
  status (on-PATH / enabled / ready) and the exact install/sign-in next step are
  shown inline, so no JSON editing or CLI flags are needed for normal Claude/Codex
  setup. The dev/test `echo` tool is no longer surfaced as a product path (it
  stays as internal smoke plumbing only). The blank/legacy-route bug stays fixed:
  the Relux shell owns every path with an in-shell not-found.
- **v0.1.0** (2026-05-23) — first standalone Relux bundle: `relux-kernel serve`
  control plane, the seven-surface dashboard (Home, Prime, Work, Crew, Plugins,
  Approvals, Health), Plugin Runtime v1 (HTTP loopback), Adapter Runtime v1
  (Claude/Codex/generic CLI, disabled by default), the safe Prime autonomy loop,
  and the deterministic tool floor (`echo` / `status`).

### Prime Autonomy Loop (First Local Version)

Relux now has a first safe autonomy loop for Prime:

- Durable config lives in the kernel snapshot/store: `enabled`,
  `interval_seconds`, `max_tasks_per_tick`, `auto_assign_unassigned`,
  `last_tick_at`, and `last_tick_summary`.
- Defaults are conservative: disabled, 60 second interval, one task per tick,
  and no auto-assignment.
- `relux-kernel serve` starts a background loop that reads the persisted config
  and runs one tick only when enabled.
- One tick uses the same governed assigned-task execution path as the Work page:
  ready assigned tasks are started, executed through the local adapter/tool path,
  completed, and audited.
- If `auto_assign_unassigned` is enabled, Prime may assign queued unassigned
  tasks to Prime before running them, capped by `max_tasks_per_tick`.
- Prime autonomy does not install/remove plugins, grant permissions, delete
  data, call paid LLMs, or bypass approvals.

CLI:

```powershell
relux-kernel prime autonomy status
relux-kernel prime autonomy enable
relux-kernel prime autonomy disable
relux-kernel prime autonomy configure --interval 60 --max-tasks 1 --auto-assign false
relux-kernel prime autonomy tick
```

API:

```text
GET   /v1/relux/prime/autonomy
PATCH /v1/relux/prime/autonomy
PUT   /v1/relux/prime/autonomy
POST  /v1/relux/prime/autonomy/tick
```

Dashboard: the Prime page exposes a Prime Autonomy panel with toggle, interval,
max tasks per tick, auto-assign, last tick summary, and a "Run one tick now"
control.

### Orchestration (First Multi-Agent Slice)

The first slice of Prime-as-orchestrator (section 10.4 Delegation Rules, section 15
"Relux can support real multi-agent workloads"). It lets Prime coordinate several
agents on one goal instead of being a single local task runner, while staying
inside the existing permission/adapter/approval model.

How it works:

- **Planning is the pure brain.** `relux_core::plan_orchestration(goal, state)`
  splits a goal into clauses on natural connectors ("then", ",", "and"),
  classifies each clause to a role (`research`, `implementation`, `testing`,
  `review`, `documentation`, `operations`, `general`), and resolves each role to a
  real agent on the live roster — matched by the agent's **id keyword OR its declared
  specialty skill** (`summary.all_agent_ids` + `summary.agent_skills`), so a
  conversational hire (`researcher`) and a manually-configured operative with an opaque
  id but a `research` skill both ground the same brief — or `None` (→ Prime fallback,
  with an honest hire note). It is conservative: a goal that does not split into ≥2
  briefs is **not** treated as multi-agent, so a greeting or a single task never
  becomes a storm (section 10.5). Step count is capped.
- **Prime classifies orchestration intent** only on explicit coordination phrasing
  ("orchestrate", "coordinate", "split this across agents", "have the team…"); a
  bare imperative still creates a single task as before.
- **Creating an orchestration** mints one brief (task) per step, assigns each to
  its agent (specialist or Prime), and records a durable `Orchestration`
  (`goal → steps[{task, agent, role, run, outcome}]`). It creates work but **does
  not run it** — nothing executes, and no paid CLI is spawned, without an explicit
  start.
- **The chat turn returns the result as STRUCTURED data**, not a wall of prose:
  the executed `Orchestration` turn carries the durable record on `PrimeTurn.orchestration`
  so the Prime chat renders a grounded result card — the goal, the ordered briefs with
  their assigned agent + role + outcome, and the planner's honest notes (which name any
  role that fell back to Prime because no specialist exists). It also attaches one-click
  `suggested_actions`: **Run this orchestration** (the explicit, governed start) and
  **Hire a `<role>` agent** for each unstaffed role (pre-filled, not auto-sent). Each
  button is an ordinary user message routed through the same grounded turn — never a
  privileged path — and showing the card runs nothing. The Work board groups the briefs
  by goal and the Crew page shows each operative's open/running brief counts, so the
  fan-out is legible across surfaces.
- **Planning infers simple dependencies.** When obvious roles co-occur in the goal
  the planner records a brief's prerequisites (`depends_on`, indices into the
  plan): **implementation waits on research**, and **testing/review/documentation
  wait on implementation**. Dependencies only ever point at *earlier* briefs, so
  the graph is a DAG by construction (no cycles, no deadlock). A goal whose roles
  do not co-occur gets no dependencies and behaves exactly as before (backward
  compatible).
- **Running an orchestration** is a governed, **dependency-aware, round-based**
  batch. Each round the scheduler (1) honestly marks any brief whose dependency
  `failed`/`blocked` as **blocked** (with a note naming the upstream brief — never
  run, never faked), (2) collects the **ready** briefs (still pending and every
  dependency `completed`), and (3) runs up to **`concurrency`** of them (clamped
  1..=4, default 2). It repeats until no brief is ready or the per-call budget
  `max` (clamped 1..=25) is spent. Each ready brief runs through **its assigned
  agent's adapter** via the same path as the Work page (`execute_assigned_run`) —
  local Prime echoes deterministically; an **enabled** Claude/Codex CLI agent
  spawns the real CLI; a disabled/unconfigured runtime or a missing permission is
  recorded as **blocked**. Each brief records its **start/finish + round**; the
  batch result reports rounds, the concurrency cap, briefs **waiting** on a
  dependency, and briefs **blocked by a failed dependency**. It runs each brief at
  most once, **stops safely** (termination is structural: every round moves ≥1
  brief to a terminal outcome, so the pending set strictly shrinks), and never
  loops, recurses, or auto-runs downloaded plugin code (section 8.2). Re-running
  only picks up still-pending briefs.
- **Concurrency:** `concurrency` bounds the *round size*, and **every path now runs
  the independent ready briefs of a round as true OS-parallel adapter processes** —
  up to the cap at once. The non-blocking job path (`run-async`, what the dashboard
  uses), the **synchronous** `POST …/run`, and the `prime orchestration run` CLI all
  drive **one shared executor**: each round splits into three phases — **prepare**
  resolves the ready set, starts each run, runs local-echo briefs inline, and
  produces spawn plans for enabled-CLI briefs (stamping run id / start / round so a
  poll sees them in flight); **spawn** runs each plan's process on its own OS thread
  concurrently; **finalize** merges each result back independently. The job path
  releases the single-owner lock around the spawn window and persists between rounds
  so a concurrent poll stays responsive; the synchronous API/CLI own the kernel for
  the whole batch (the API on the blocking pool, the CLI as a one-shot process), so
  two concurrent runs can never double-execute a brief. All governance stays under
  the lock (permission + runtime gating before any spawn, redaction, transcript,
  audit, retry) and **no downloaded plugin code is ever auto-run** (only an explicitly
  enabled, operator-configured local binary spawns). Each brief runs **at most once
  per round**, a failure/panic in one brief never corrupts a sibling, and
  dependencies still gate future rounds. The synchronous `/run` and CLI **block until
  the whole batch is done** and return the final result; `run-async` returns a job id
  immediately and is polled for live progress.

This is distinct from the background autonomy loop above, which stays deterministic
(echo-only) and never spawns a paid CLI. Orchestration is operator-triggered.

CLI:

```powershell
relux-kernel prime orchestrate "research the options, implement a prototype, and write the docs"
relux-kernel prime orchestration list
relux-kernel prime orchestration show <id>
relux-kernel prime orchestration run <id> [--max N] [--concurrency N]
```

API:

```text
POST /v1/relux/prime/orchestrate/preview      # preview a plan, commit nothing
POST /v1/relux/prime/orchestrations           # create (plan + assign) from { goal }
GET  /v1/relux/prime/orchestrations           # list
GET  /v1/relux/prime/orchestrations/:id       # one record + full step chain
POST /v1/relux/prime/orchestrations/:id/run   # governed dependency-aware batch ({ max?, concurrency? }), blocking
POST /v1/relux/prime/orchestrations/:id/run-async  # start a NON-BLOCKING background job; returns { ...job, status_url } immediately
GET  /v1/relux/prime/orchestrations/:id/job   # the latest job for this orchestration (poll by orchestration id)
GET  /v1/relux/orchestration-jobs/:job_id     # poll one job: state queued/running/completed/failed/canceled + round/step statuses/result
POST /v1/relux/orchestration-jobs/:job_id/cancel  # request cooperative cancellation; 200 + updated job, 404 unknown, 409 already finished
```

Dashboard: the Prime page has an **Orchestration** panel (goal → preview plan →
create → run/continue, with per-agent briefs and outcomes). The preview shows each
brief's inferred dependencies; each orchestration shows a dependency-aware
readiness line (how many briefs are **ready** now vs **waiting** on a dependency vs
**blocked**), per-brief derived lifecycle badges (ready/waiting on a pending
brief), the **round** each brief ran in, and the last batch's rounds + concurrency
cap. **Run/Continue now starts a non-blocking background job and polls it:** the
button kicks off `run-async`, then a 1s poll loop renders the live phase
("Queued" → "Running — round N" → "Completed"/"Failed"), a running tally
(`ran/total briefs · completed · failed · blocked`), the worker's last event, and
a real **running** badge on the brief(s) executing this round (taken from the job's
step snapshot, never a guessed spinner). The button stays disabled while the job is
active so a second click can't start a duplicate (the backend also rejects it).
While a job is active the panel also shows a **Cancel** button: pressing it
requests cooperative cancellation (`POST …/orchestration-jobs/:job_id/cancel`), the
phase label flips to "Canceling — finishing round N", and once the worker stops the
job shows **Canceled**. On completion (or cancellation) the panel folds the job's
aggregate result into the "Last batch" banner and refreshes the durable record.
Home shows the newest unfinished orchestration with its progress and next action.
Pure UI logic lives in `apps/dashboard/src/orchestration.ts` (job helpers:
`jobIsActive` / `jobIsTerminal` / `jobIsCanceling` / `jobCanCancel` /
`jobPhaseLabel` / `jobProgressLabel` / `jobRunningStepIds` / `runButtonLabel`) with
unit coverage in `apps/dashboard/test/orchestration.test.ts`.

Progress visibility is now honestly **live**: a `run-async` job runs on a
background thread that drives the SAME governed, tested `run_orchestration` one
round at a time — releasing the single-owner kernel lock and persisting the
orchestration record **between** rounds — so polling the job (or the durable
record) sees real, already-recorded per-brief start/finish/round and the
dependency-aware ready/waiting/blocked state **as the batch progresses**, not only
after it returns. The blocking `/run` endpoint stays for the CLI/tests. Two honesty
contracts hold: (1) the briefs about to run this round are reported as `running`
from the durable readiness rule — nothing fabricates in-flight progress; (2) the
job registry is **in-memory only**, so a server restart mid-job loses the live job
record — but a poll **by orchestration id** (`GET …/orchestrations/:id/job`) stays
**restart-honest** by *reconstructing* a job-like status from the durable record
when no live job exists: `completed` when every brief is terminal, else
`interrupted` (a prior worker ran but is gone; pending briefs remain and can be
resumed with a fresh run), with a clearly-synthetic `durable:<id>` id and a message
explaining the pending work. Reconstruction fabricates nothing — every field comes
from what the kernel already persisted (per-brief outcomes, run ids, rounds); an
orchestration that never ran a brief still honestly 404s ("no job started") so the
dashboard shows its planned record. Only the raw **by-job-id** endpoint
(`GET …/orchestration-jobs/:job_id`) 404s for a lost job, because process-local job
ids cannot be mapped to an orchestration after a restart — its 404 message points
the caller at the durable by-orchestration-id poll. The worker never spins: each
round moves ≥1 brief to a terminal outcome and it stops as soon as a round runs no
brief, the per-job budget is spent, or the orchestration is no longer `running`.
Duplicate starts are rejected (409, one active job per orchestration) and the fleet
is capped (429 past the operator-configured admission limit —
`PrimeAgentPolicy::max_active_jobs`, default 4, or `extended_max_active_jobs` when the
start opts into `{"extended": true}`, both clamped to a 64 ceiling; the 429 names the
configured limit and how to raise it, never a hidden constant).

**Cancellation is cooperative and honest.** A cancel request sets a flag the worker
checks **between** rounds (where the kernel lock is free and the prior round has
fully persisted). It does **not** kill an adapter process mid-flight: the round that
is already running finishes — every brief in it keeps its real recorded outcome —
and the worker then stops *before* the next round and marks the job `canceled`. The
remaining briefs are left in their durable (pending) state, so a human can resume
with a fresh run later (a canceled job is terminal and no longer blocks a new one).
The cancel endpoint only sets the flag; the worker owns the `canceled` state
transition, so cancellation never races the worker on the state field. A cancel that
arrives too late (the job finished its rounds first) leaves the job `completed` —
never a faked cancellation. Backend job lifecycle/duplicate/cap/aggregate **and the
cancel state machine + the cooperative worker stop (with a positive control proving
the same plan runs to completion without a cancel)** are unit-tested in
`crates/relux-kernel/src/server.rs`; an end-to-end HTTP smoke
(`scripts/smoke-orchestration-job.ps1`, plus a real-Claude-CLI variant
`scripts/smoke-orchestration-job-claude.ps1`) proves the start → poll → terminal
path against a live kernel. A dedicated **live mid-flight cancel** smoke
(`scripts/smoke-orchestration-cancel.ps1`) closes the last gap: it routes the first
brief to a deliberately slow local CLI adapter (a fake `ping`-based command spawned
through the **real** adapter path, not test-only internals), polls until that brief
is genuinely `running`, requests cancel, observes `cancel_requested` while the job is
still `running` (the canceling phase), then asserts the terminal `canceled` state
with the in-flight brief recorded `completed` honestly and every downstream brief
left `pending`. A companion **multi-brief in-flight cancel** smoke
(`scripts/smoke-orchestration-cancel-multi.ps1`) hardens the honesty contract for the
case it really hinges on — a cancel that lands while **two** independent briefs are
running together in the same round: it routes a research brief and an operations
brief to two separate slow local CLI adapters (both spawned through the real adapter
path), runs the job at `concurrency=2`, polls until a single snapshot shows **both**
briefs `running`, requests cancel, observes `cancel_requested` while still `running`,
then asserts the terminal `canceled` state with **both** in-flight briefs recorded
`completed` honestly and the downstream implementation + documentation briefs left
`pending`.

**Resume-after-cancel is genuine, not a promise.** The "left `pending` for a human to
resume with a fresh run" claim above is now proven. Because the duplicate-job guard
only rejects a `queued`/`running` job for the same orchestration (a terminal
`canceled` job no longer counts) and a round only schedules `pending` briefs whose
dependencies are `completed`, starting a fresh job on a partially-done orchestration
resumes it exactly where the cancel stopped — already-completed briefs are skipped, not
re-run. This needs no special resume code; it falls out of the durable record being the
single source of truth. A deterministic unit test
(`a_second_job_resumes_only_pending_briefs_and_preserves_completed_runs`) and a
dedicated **live resume-after-cancel** smoke (`scripts/smoke-orchestration-resume.ps1`)
pin it: the smoke runs the multi-brief cancel scenario, then starts a fresh job on the
same orchestration and asserts it is accepted (not a 409), runs **only** the previously
`pending` downstream briefs (`job.ran` equals the pending count — the completed round-1
briefs are never re-run), preserves each completed brief's original run id and round,
gives each resumed brief a brand-new run id, and drives the record to fully `completed`.

**Job status is restart-honest.** Because the registry is in-memory, a server
restart loses every live job — but the durable record outlives it, so a poll **by
orchestration id** (`GET …/orchestrations/:id/job`) reconstructs an honest job-like
status when no live job exists: `completed` when every brief is terminal, else
`interrupted` (a prior run left briefs pending and no worker is driving it now),
with a synthetic `durable:<id>` id and a `ran` count that matches the record.
Reconstruction (`reconstruct_job_from_record`) fabricates nothing and returns
`None` (an honest 404) for an orchestration that never ran a brief, so the dashboard
falls back to the planned record. The raw **by-job-id** poll still 404s for a lost
job (process-local ids can't be remapped after a restart) with a message pointing at
the durable poll. Unit tests
(`reconstruct_returns_none_when_no_brief_ever_ran`,
`reconstruct_reports_interrupted_after_partial_run_across_restart`,
`reconstruct_reports_completed_when_all_briefs_terminal_across_restart`) pin the
reconstruction over a fresh registry on the same store, and a dedicated **restart**
smoke (`scripts/smoke-orchestration-restart.ps1`) proves it end-to-end against a
kernel that is genuinely stopped and restarted: a `max=1` job leaves briefs pending,
restart #1 reconstructs `interrupted` (and the lost job id 404s), a fresh job
resumes to `completed`, and restart #2 reconstructs `completed`. The dashboard treats
`interrupted` as terminal (`jobIsTerminal`) with an honest phase label, so it stops
polling, shows the durable progress, and re-enables Continue to resume. The
orchestration panel renders this as a **distinct restart-honest callout** (separate
from the live-job banner): it labels the status as reconstructed from the durable
record — explicitly *not* a live run — surfaces the completed-vs-pending split, and
points at Continue to resume only the pending briefs. It detects a reconstructed
status by the synthetic `durable:<id>` (`jobIsReconstructed`) and never presents that
id as a live worker. So a reload after a restart still surfaces the callout (not only
the session that pressed Run), the panel **hydrates** the durable job status once on
load for any `running` orchestration — which also reconnects to a still-live job — and
relies on the terminal gate so a reconstructed status schedules no further polling.

**Per-brief timing is surfaced, honestly.** Because every brief carries the recorded
`started_at`/`finished_at` from the kernel's logical clock, the brief detail now shows
each brief's **recorded run duration** next to its round — the elapsed `finished −
started`, formatted by the same single duration formatter the run view uses
(`stepDurationLabel`). It only ever shows a *measured, terminal* duration: a brief that
started but has not finished shows nothing (no fabricated live timer, consistent with
the in-flight honesty contract), and an unparseable or backwards stamp pair is dropped
rather than rendered as a wrong number. The interrupted-UX **render harness** proves the
callout + Continue button actually render and ship (server-rendered real component +
committed-bundle copy assertion); the one binding it does not cover — the browser click
from Continue to the resume request — is deliberately **not** closed with a browser
toolchain, because the resume itself is already proven end-to-end by the
resume-after-cancel / restart unit tests and smokes, leaving only a one-line event
binding not worth a 100s-of-MB engine download (see `apps/dashboard/README.md`).

### Tool Invocation Surface (First Honest Version)

Installed ToolSet plugins are now visible, callable capabilities through the
kernel, CLI, API, and dashboard - the first step toward Prime as a Codex-like
operator with plugin abilities (sections 7.4, 8.2, 9.8).

The first version is safe and honest by construction:

- Only the kernel's **built-in deterministic tool handlers** execute. Two ship
  today: `relux-tools-echo` / `echo.say` (returns input unchanged) and
  `relux-tools-status` / `status.summary` (read-only control-plane counts). The
  single source of truth for what is runnable is `relux_kernel::builtin`.
- An installed tool with no built-in runtime is discoverable but reported as
  `not_implemented` - it is never faked. Invoking it returns a clear
  `ToolRuntimeUnavailable` error (HTTP 501) with no fabricated output.
- Arbitrary downloaded plugin code (GitHub/zip/folder installs) is installable as
  metadata/manifests but is **not executed**. No shelling out to plugin commands,
  no filesystem/network side effects from installed plugins.
- Every invocation routes through the kernel permission check and is written to
  the audit log (success, denial, or not-implemented). Nothing bypasses the
  kernel. A permission denial returns HTTP 403.
- `KernelState::call_tool` runs inside a run (transcript + audit);
  `KernelState::invoke_tool` is the run-free audit-only path behind the API/CLI;
  `KernelState::discover_tools` powers capability discovery, optionally scoped to
  one agent's permissions (`ready` / `not_implemented` / `missing_permission`).

CLI:

```powershell
relux-kernel tools
relux-kernel tool invoke relux-tools-echo echo.say '{"message":"hi"}'
relux-kernel tool invoke relux-tools-status status.summary
```

API:

```text
GET  /v1/relux/tools            # installed tools + executable status (?agent=<id> to scope)
POST /v1/relux/tools/invoke     # { "plugin_id", "tool_name", "input"?, "agent_id"? }
```

Dashboard: the Plugins page lists installed tools with an honest executable
status and offers a small invoke panel (JSON input + output/error) for ready
tools. An installed tool with no runtime shows "installed, runtime not
implemented yet" rather than being hidden or pretend-run.

### Plugin Runtime v1 (HTTP loopback ToolSet runtime)

Installed ToolSet plugins can now become executable through an **explicitly
configured loopback HTTP endpoint** (§8.2, §18: Relux does not auto-run
downloaded plugin code). Relux still never shells out to plugin commands, never
runs code from GitHub/zip/folder installs in-process, and never calls a remote
host. Instead, the plugin author/operator runs their own local server and opts a
plugin into execution by configuring a loopback URL for it; Relux calls that
server through a narrow, permission-checked, audited protocol.

Protocol (one stable endpoint):

```text
POST <base_url>/invoke
Content-Type: application/json
{ "plugin_id": "...", "tool_name": "...", "input": <json> }

200 { "output": <json> }   -> success
200 { "error": "..." }     -> the tool refused/failed (surfaced honestly)
```

Safety (enforced by the kernel):

- Loopback only: `http://127.0.0.1|localhost|[::1]:<port>` with an explicit port.
  `https`, remote hosts, embedded credentials, query/fragment, and `..` paths are
  rejected (`relux_core::validate_loopback_url`, re-validated on every call).
- Per-call timeout (default 5000 ms, clamped 100-60000), request/response body
  caps, JSON-only. No TLS, no redirects.
- Every invocation routes through the SAME kernel permission check + audit path as
  the built-ins; a connect failure, timeout, non-200, oversized body, invalid
  JSON, or `{ "error": ... }` becomes a clear error, never a fabricated success.
- The per-plugin config is persisted locally and stores NO secrets (only the base
  URL, enabled flag, timeout). Bundled plugins cannot be given a loopback runtime.

Tool discovery now reports `ready` (built-in or enabled loopback runtime),
`runtime_not_configured` (installed, no runtime yet), `runtime_disabled`, or
`missing_permission`. `not_implemented` is reserved for a tool with no supported
runtime at all.

CLI:

```powershell
relux-kernel plugin runtime <plugin-id>
relux-kernel plugin runtime set <plugin-id> <base-url> [--timeout-ms N]
relux-kernel plugin runtime disable <plugin-id>
```

API:

```text
GET    /v1/relux/plugins/:id/runtime
PUT    /v1/relux/plugins/:id/runtime    { "base_url", "enabled"?, "timeout_ms"? }
PATCH  /v1/relux/plugins/:id/runtime    (partial update)
DELETE /v1/relux/plugins/:id/runtime
```

`/v1/relux/tools` reflects the runtime status and `/v1/relux/tools/invoke` routes
configured loopback tools through the runtime client. Dashboard: each non-bundled
plugin on the Plugins page has a Runtime panel (set loopback URL + timeout,
disable, clear); configured tools show as `ready` and are invokable from the
existing invoke panel.

Prime chat (§10, §11.1): Prime is now tool-aware and can list/invoke the safe
built-in tools directly from chat, so simple tool use does not require leaving
Prime for the Tools panel. Two new intents drive this - `tool_discovery`
("what tools can you use?" → grounded `discover_tools`, never a fabricated list)
and `tool_invocation` ("echo hello", "use echo.say with {json}", "run the status
tool"). A status question also grounds itself by consulting
`relux-tools-status/status.summary`. Every Prime tool call routes through
`KernelState::invoke_tool` - the SAME permission/audit path as
`/v1/relux/tools/invoke` - and the turn carries structured `invoked_tool` /
`tool_output` / `tool_error` fields. Prime stays honest: a greeting never becomes
a tool call; an installed-but-unimplemented tool is reported as not runnable here
(no fabricated output); a missing permission is surfaced, never bypassed; and
**arbitrary downloaded plugin runtime execution remains intentionally not
implemented.**

### Plugin Install UX v1 (honest metadata-only wrappers)

**A `relux-plugin.json` is never required to import a repo.** That file is *optional* —
only first-class Relux plugins ship one, and almost no external repo will. Paste any
GitHub repo / folder / zip and it succeeds as a **generated metadata-only wrapper**
(sanitized id, zero tools, zero permissions, `Unverified`, `generated: true`). The
install surface copy says this plainly — headline **"Install any GitHub repo — no Relux
manifest needed"**, the manifest framed as optional throughout, and the post-install
result reads **"Imported as metadata-only — no Relux manifest needed"** (never a failure
or "manifest required"). The dashboard then makes that state honest and actionable
instead of leaving the operator to wonder:

- **No "ready"-looking label.** A wrapper is badged **Needs configuration** (amber),
  never the green "enabled" a real plugin shows. Its row carries an inline banner
  stating the dead-end plainly: it declares no tools, so a runtime alone runs
  nothing.
- **The honest next step is a tool definition, not a runtime.** Because
  `discover_tools` only surfaces manifest-declared tools, a wrapper with no tool
  definitions stays empty even with an enabled loopback runtime (pinned by the
  kernel test `enabling_a_runtime_on_a_wrapper_surfaces_no_tools`). So the wrapper's
  call to action is **Configure → add a tool definition**, not "configure a
  runtime". The Configure panel now offers an **in-UI add-a-tool form** (see *Plugin
  Tool Config v1* below) as the default; the prior copy/download
  `relux-plugin.json` + re-install path is kept as an "Advanced" fallback. Once a
  tool exists the row also exposes the loopback **Runtime** panel. Relux still never
  infers tools from repo content and never runs downloaded code.
- **Plugin categories are distinct.** The Kind column distinguishes **Adapter**
  (configured on the Crew page), **ToolSet** (with its declared tool count and a
  loopback **Runtime** panel), **Metadata-only wrapper** (Set up → manifest), and
  internal dev/test plugins (hidden by default, §echo). A real manifest-based
  plugin is unaffected and keeps its normal Runtime flow.
- **Install result summary.** After an install the panel stays open and reports
  what happened — tools discovered (count), an import as metadata-only (nothing
  runnable yet, *no manifest needed*), or an adapter installed — and the exact next
  step (review detected hints, register an MCP server, or add a tool definition),
  instead of a bare "Installed X". The result line carries `generated` + `tool_count`
  from the install API so the wrapper-vs-native distinction is shown, not guessed.
- **Runnable-only tools by default.** The Tools list shows only `ready` tools by
  default so a metadata-only or unconfigured plugin never looks usable; a
  "Show N non-runnable" toggle reveals the rest with their honest status. Nothing
  is permanently hidden or faked.

The pure UI derivations (status, kind label, next-step, install summary, tool
visibility) live in `apps/dashboard/src/plugins.ts`, unit-tested in
`apps/dashboard/test/plugins.test.ts`. The backend adds `tool_count` to each
`/v1/relux/plugins` record and a read-only template endpoint:

```text
GET /v1/relux/plugins/:id/manifest-template
  -> { plugin_id, filename, install_dir, generated, manifest_json }
```

The returned `manifest_json` is a complete, re-installable starter `relux-plugin.json`
(ToolSet, one example tool, permission strings bound to this plugin id). It stores
nothing and runs nothing — it is guidance the operator fills in. Covered by kernel
tests `generated_wrapper_record_is_flagged_and_has_zero_tools`,
`real_toolset_record_reports_its_tool_count`, and
`manifest_template_is_valid_json_keyed_to_the_plugin`.

### Plugin Tool Config v1 (in-UI tool definitions for a wrapper)

The first **safe in-UI path to make a metadata-only wrapper useful**: instead of
hand-editing `relux-plugin.json` and re-installing, the operator opens **Configure**
on a user-installed ToolSet/wrapper row and adds ONE tool at a time through a small,
validated form. See `docs/reference-driven-development.md` → *Reference read — safe
in-UI tool configuration for a metadata-only wrapper* for the openclaw patterns this
mirrors (`readPlanSteps` field-by-field + status-allowlist validation,
`sessions-spawn-tool` unsupported-key rejection + clamps, `readStringParam`
required-throws).

```text
POST   /v1/relux/plugins/:id/tools        { name, description?, risk?, auto_approve?, timeout_secs? }
DELETE /v1/relux/plugins/:id/tools/:tool
```

Safety contract (all fail-closed, validated in
`crates/relux-kernel/src/plugin_tool_config.rs` + `state.rs`
`configure_plugin_tool`/`remove_plugin_tool`):

- **Only an installed, non-bundled `ToolSet`** (a generated wrapper is a ToolSet) is
  editable. A bundled/protected fixture and a non-ToolSet plugin (adapter, …) are
  refused (409 / 400). The manifest is mutated transactionally on a clone and
  re-validated with `validate_manifest` before it stands, then persisted through the
  install store (authoritative for a user plugin; the bundled refresh never touches
  it).
- **The operator never supplies a raw permission.** The kernel DERIVES it as
  `tool:<plugin-id>:<verb>` from the (sanitized, dotted) tool name, so a configured
  tool can only ever gate on this plugin's own `tool:` namespace. Allowlist fields
  only (`name`/`description`/`risk`/`auto_approve`/`timeout_secs`); any other key
  fails the whole payload closed. `risk` is validated against the `RiskLevel`
  allowlist; the timeout is clamped to `[1, 300]`s.
- **Risk-driven, load-bearing approval.** `risk == low` → auto-approvable (the
  operator opts in); any non-low risk is `approval: Required`. That approval is now
  ENFORCED at tool execution (previously the field was decorative):
  `relux_core::approval_blocks_direct_invocation` backs a new
  `ToolExecutability::NeedsApproval` discovery status and a refusal in
  `call_tool`/`invoke_tool`, so a non-low-risk tool is never runnable just because a
  loopback runtime is enabled. All bundled fixtures declare `approval: never`, so
  their behavior is unchanged.
- **A tool is still not runnable until the operator enables a loopback runtime**
  (the separate, explicit run-enabling step) and the calling agent holds the derived
  permission. Adding a tool only makes it *discoverable* + honestly statused
  (`runtime_not_configured` until a runtime is enabled; `needs_approval` for a
  gated risky tool).

Covered by kernel tests `configure_plugin_tool_adds_a_validated_tool_to_a_wrapper`,
`a_non_low_risk_tool_needs_approval_and_is_refused_directly`,
`configure_plugin_tool_refuses_bundled_and_unknown_plugins`,
`remove_plugin_tool_drops_the_tool_and_its_unused_permission`,
`configuring_a_tool_on_a_wrapper_makes_it_appear_and_bumps_the_record`,
`tool_config_error_status_codes_are_honest`, the `plugin_tool_config` parser tests,
and the `relux-core` `tool::tests` (approval predicate). The dashboard form lives in
`apps/dashboard/src/pages/Plugins.tsx` (`ManifestPanel` → `AddToolForm` /
`ConfiguredToolsList`); the `canConfigureTools` derivation + tool-count-aware status
live in `apps/dashboard/src/plugins.ts`, unit-tested in
`apps/dashboard/test/plugins.test.ts`.

#### Tool invocation workflow + honest readiness (UI/API)

The end-to-end operator workflow for a generated wrapper, all on the **Plugins**
page (no separate Tools route — the Tools list and its actions are inline, so a
non-ready tool never opens a blank page):

1. **Install** the source → a metadata-only wrapper (no tools, nothing runnable).
2. **Configure a tool** (Plugin Tool Config v1 above) → the tool becomes
   *discoverable*, statused `runtime_not_configured`.
3. **Enable a loopback runtime** (`PUT /v1/relux/plugins/:id/runtime`, a
   `http://127.0.0.1|localhost|[::1]:<port>` base URL) → a **low-risk** tool flips
   to `ready` once the calling agent holds the derived permission.
4. **Invoke** — `POST /v1/relux/tools/invoke { plugin_id, tool_name, input?,
   agent_id? }`, or the inline JSON-input form on a `ready` tool row. The call runs
   the same permission gate → approval gate → runtime as the CLI, is audited, and
   returns a structured `ToolInvocationResult { output }`. The runtime itself
   (`crates/relux-kernel/src/runtime.rs`) is **loopback-only, JSON-in/JSON-out,
   bounded** (256 KiB request cap, 1 MiB response cap, per-call connect/read/write
   timeout clamped to `[1,300]`s, no redirects/TLS/streaming); a connect/timeout/
   non-200/oversized/invalid-JSON/`{"error":…}` response is an honest
   `KernelError`, never a fabricated success. Relux never shells out to plugin
   commands or runs downloaded plugin code in-process (§18).

**Guided Setup checklist (the workflow, made visible).** The four steps above are
not just prose — for a configurable metadata-only import the **Configure tools**
panel now opens with a **Setup checklist** that PRESENTS that exact documented
order with each step's live status, so an operator never has to guess it (and never
reaches for a runtime first, which on a wrapper surfaces nothing). The step statuses
are derived purely from real state by `apps/dashboard/src/plugins.ts`
`guidedConfigSteps(plugin, tools, runtime, hints)` → 1 **Review the imported
source** (done once the source was scanned), 2 **Add a tool definition** — titled
*Register an MCP server, or add a tool* when an `mcp_proposal` is present, 3
**Enable a loopback runtime** (`upcoming` until a tool exists; `current` once one
does; `done` when configured **and** enabled), 4 **Use it from Prime or the Work
board** (`done` only once a tool reports `ready`). A step is **actionable only when
the backend already supports it**; when it is not, the step carries an honest
`Needs:` line naming the missing prerequisite (e.g. a non-configurable bundled
plugin's define step is `blocked` with *can't be configured in-UI*). The checklist
invents no authority — every action is one of the existing gated panels below it
(`GuidedConfigChecklist` reads runtime via `GET /v1/relux/plugins/:id/runtime` and
the lifted hints fetch; it never registers, enables, or runs anything itself). The
in-UI **Add a tool** form is also honest that a tool definition is
name/description/risk/timeout **only** — there is no input-schema field; the
loopback server receives the JSON input passed at call time. Pinned by
`apps/dashboard/test/plugins.test.ts` (`guidedConfigSteps` per-state assertions) and
`apps/dashboard/test/install-result-render.test.mjs` (the wrapper result renders the
4-step checklist + the "no input-schema field" copy).

**Honest readiness in the UI.** `apps/dashboard/src/plugins.ts` `toolReadiness`
is the single classifier (mirroring openclaw `acp/approval-classifier.ts` — one
function, a named class, only the safe class is runnable) that maps the kernel's
`executable` status to what the operator sees. ONLY `ready` is runnable (an
Invoke form); every other state renders a clear, non-blank **"Why not?"** panel
with the honest reason + next step — `needs_approval` (refused on the direct path;
the operator may instead request a per-call approval, see below),
`runtime_not_configured`, `runtime_disabled`, `missing_permission`,
`not_implemented`. This is the same refusal the kernel enforces in
`call_tool`/`invoke_tool`, just rendered honestly — a gated tool is never shown as
runnable and the UI never pretends a refused tool ran. Pinned by
`apps/dashboard/test/plugins.test.ts` (`toolReadiness` per-state assertions) and
the kernel tests above.

#### Source introspection hints (what an imported repo contains)

A metadata-only wrapper is honest that *nothing runs yet*, but the operator still
needs to know *what the source is* to wire it up. `GET /v1/relux/plugins/:id/hints`
(`crates/relux-kernel/src/introspect.rs` `detect_hints`) performs a **read-only,
never-executed** scan of the installed directory and returns informational
**hints** only. Safety bounds, all enforced by construction:

- The directory is scanned **only when it lives inside the plugins root**, so an
  arbitrary path can never be inspected through this route (a bundled fixture under
  `examples/` reports `scanned: false`, no hints).
- The scan reads only a fixed allow-list of top-level metadata files
  (`package.json`, `pyproject.toml`/`setup.py`/`requirements.txt`, `mcp.json`,
  `Dockerfile`, `Cargo.toml`, `Makefile`, README, top-level `*.py`/`*.sh`), each
  capped at `MAX_FILE_BYTES`, with the hint list capped at `MAX_HINTS`. It **never
  spawns a process, follows a command/entrypoint, recurses, or promotes a hint into
  a tool**.
- Signals detected: a **possible MCP server** (the `@modelcontextprotocol/sdk`
  dependency in `package.json`, an `mcp`/`fastmcp` dependency in
  `pyproject.toml`/`requirements.txt`, or a standalone `mcp.json`), an **npm/python
  package**, declared **bin/entrypoints**, a **container/Rust crate**, **scripts**,
  and a **README**.

The Configure panel (`DetectedHints` in `Plugins.tsx`, labels/next-step in
`plugins.ts` `hintKindLabel`/`hintsNextStep`) shows these under a **"hints only"**
badge with an advisory next step — never a claim that anything is runnable.

**Converting an imported repo into a real plugin / tool / MCP config.** The hints
tell the operator which of the existing governed paths to take; Relux runs nothing
on their behalf:

1. **If the source is an MCP server** (an `mcp-server`/`mcp-config` hint): the plugin
   details now offer a one-click **"Register MCP server…"** action. The same hints
   scan builds a read-only `McpRegistrationProposal`
   (`crates/relux-kernel/src/mcp_proposal.rs` `propose_mcp_registration`) — a
   **sanitized, valid** server id (`relux_core::sanitize_mcp_server_id`), a
   description, and a loopback endpoint **only when an `mcp.json`'s `url` passes the
   loopback rule** (a detected stdio `{command, args}` is shown as advisory text only,
   never run, never the endpoint; otherwise manual entry is forced). The action opens a
   **pre-filled review form** (`DetectedHints` → `AddMcpServerForm`); the operator
   confirms/edits and submits to the **existing** `POST /v1/relux/mcp/servers`
   (loopback-only) — no parallel registry, nothing auto-registered, nothing executed.
   After registering, **Discover** lists its tools through the same per-call gate as
   any MCP tool (an unclassified tool stays gated). Relux never launches it for you.
2. **If the source is a CLI / script / binary** (a declared npm `bin`, a Python
   `[project.scripts]`, a Cargo `[[bin]]`): the detected `command_tool` candidate now
   offers a one-click **"Configure (command tool)…"** review form. Confirm the argv
   recipe and submit to `POST /v1/relux/plugins/:id/command-tools`; the tool becomes
   `needs_approval` (gated) and runs argv-only, confined to the install dir, only on a
   gated invocation — see "Governed command tools" below. Relux never runs it on import.
3. **If the source is a package/entrypoint** that exposes an HTTP surface: run it
   yourself as a local **loopback** server, then on the Plugins page add a tool
   definition (Plugin Tool Config v1) and point a loopback runtime at it
   (`PUT /v1/relux/plugins/:id/runtime`). The tool becomes `ready` only after that
   explicit operator action and the existing permission/approval gates.
4. **If you have a real manifest**, author `relux-plugin.json` from the
   manifest-template (`GET /v1/relux/plugins/:id/manifest-template`) and re-install
   — the preferred path, used directly with no scaffolding.

Pinned by `crates/relux-kernel/src/introspect.rs` unit tests (npm+MCP, python+MCP,
mcp.json, container/rust/scripts/readme, oversized-file skip, hint-count bound),
`crates/relux-kernel/src/mcp_proposal.rs` unit tests (npm-SDK → safe id + manual
endpoint, loopback `mcp.json` `url` → pre-filled endpoint, remote `url` → manual,
stdio `command` → advisory-only never an endpoint, id fallback, no-signal → no
proposal), `crates/relux-core/src/mcp.rs` `sanitize_server_id_yields_a_valid_id_or_empty`,
the server tests `hints_route_introspects_an_imported_repo_without_a_manifest` (now also
asserting the pre-filled proposal) / `hints_route_does_not_scan_outside_the_plugins_root`
(no proposal when nothing scanned), and the dashboard
`hintKindLabel`/`hintsNextStep`/`mcpDraftFromProposal`/`validateMcpRegisterDraft` assertions.

##### Structured capability candidates (install → usable, per capability)

Flat hints + a single MCP proposal answer "what is this source?" but an ordinary repo
(a CLI tool, a Python/Cargo script, several entrypoints) still got no concrete,
per-capability path. `GET /v1/relux/plugins/:id/hints` now additionally returns a
**`candidates`** array (`crates/relux-kernel/src/capability_detect.rs`
`detect_candidates`) built from the SAME read-only scan — it executes nothing, reads
only the same bounded metadata files, and never promotes a candidate into a runnable
tool. Each `CapabilityCandidate` carries a `kind` (`mcp_stdio` | `mcp_http` |
`cli_command`), an honest `confidence` (`high`/`medium`/`low`), a `risk` band, a
one-line `rationale` (the exact signal matched), a non-secret `command_preview`, any
required `env_placeholders` (names only — never values), and an `activation`:

- **`mcp_register` — one-click governed activation.** The only shape Relux can turn
  into a usable capability today. The candidate carries a pre-filled
  `McpRegistrationProposal` for the unchanged loopback-only `POST /v1/relux/mcp/servers`
  review form (managed-stdio for a stdio command, HTTP for a loopback `url`). An npm
  `@modelcontextprotocol/sdk` source with a declared `bin` is **enriched** into a
  reviewable `node <bin>` managed-stdio draft — surfaced for review, never run by
  detection; managed-stdio re-validates + spawns it argv-only only after the operator
  registers and Discovers. MCP candidates lead the list.
- **`command_tool` — a governed argv activation.** A detected CLI binary / Python or
  Cargo script for which a concrete command could be inferred. The candidate carries a
  safe, pre-filled `command_tool` argv draft (`{ tool_name, program, args, cwd? }`,
  split from the non-secret `command_preview` — display text, never run by detection)
  for the `POST /v1/relux/plugins/:id/command-tools` review form. The operator
  confirms/edits the recipe and submits; the kernel validates the **argv safety
  contract** (`relux_core::validate_command_tool_config` — argv-only, no shell
  metacharacters, no danger flag, bounded, no `..` cwd), derives the permission, adds a
  manifest `ToolDefinition` (approval **always Required**), and stores a
  `relux_core::CommandToolConfig`. Nothing runs at configure time. The command executes
  only later, through a **gated invocation** (or a standing grant), argv-only, confined
  to the plugin's install directory, with a timeout and bounded, secret-redacted output
  (`crates/relux-kernel/src/command_exec.rs`). This is the activation path that turns an
  ordinary repo script into a real, safe Relux tool — see "Governed command tools" below.
- **`manual` — an honest pending capability.** A detected entrypoint for which **no**
  concrete command could be inferred. No fake "ready": the candidate carries concrete
  `next_steps` through the existing governed paths (register an MCP/stdio interface, run
  it as a loopback server + add a tool definition, or author a `relux-plugin.json`).

When the source was scanned but produced **no** candidate, the dashboard shows exact
"what to add" guidance (an `mcp.json` / an MCP dependency / a loopback server + tool
definition / a manifest) — never a dead end. The Configure panel renders
`DetectedCapabilities` → `CapabilityCard` (`apps/dashboard/src/pages/Plugins.tsx`),
with presentation derived purely by `apps/dashboard/src/plugins.ts`
(`candidateKindLabel`/`candidateConfidenceBadge`/`isOneClickCandidate`/
`isCommandToolCandidate`/`mcpDraftFromCandidate`/`commandToolDraftFromCandidate`/
`capabilitySummary`). An `mcp_register` candidate's Configure opens the SAME pre-filled
`AddMcpServerForm` the MCP-hint path uses; a `command_tool` candidate's Configure opens
the `ConfigureCommandToolForm` posting to `POST /v1/relux/plugins/:id/command-tools` —
both route through identical, unchanged validation; no parallel registry, nothing
auto-registered or auto-run. Pinned by `crates/relux-kernel/src/capability_detect.rs`
unit tests (npm-MCP+bin → one-click managed-stdio, env keys → placeholders not values,
pyproject/cargo/npm bin → `command_tool` with a pre-filled argv draft, README-only → no
candidate, candidate-count bound, slug safety), the server tests
`hints_route_introspects_an_imported_repo_without_a_manifest` /
`command_tools_route_activates_a_detected_cli_candidate` /
`hints_route_does_not_scan_outside_the_plugins_root`, the dashboard `plugins.test.ts`
candidate-helper assertions, and `apps/dashboard/test/install-to-usable-render.test.mjs`
(one-click vs command-tool vs manual rendering, env name not value, empty-after-scan).

##### Governed command tools (an imported CLI/script/binary → a real, gated tool)

MCP is the one-click path; **governed command tools** close the other half — turning a
detected `cli_command` candidate (or any operator-described repo script/binary) into a
real, callable Relux tool **safely**, without hand-writing JSON. This is the smallest
production-shaped path that fits the existing tool architecture (no parallel universe):

- **Data model (`relux_core::command_tool`).** A `CommandToolConfig` stores the command
  as an `argv` `program` + fixed `args` + an optional `cwd` (relative to and confined
  within the plugin's install dir) + declared optional `input_args` + a clamped
  `timeout_ms` + an `enabled` flag. **There is no shell-string field**, so there is no
  metacharacter-injection surface. `validate_command_tool_config` reuses the SAME
  `validate_stdio_command` the managed-stdio MCP transport uses (argv-only, no shell
  metacharacters in the program token, bounded control-char-free args, no
  `--yolo`/bypass danger flag); `build_command_argv` appends ONLY declared `input_args`
  (each a bounded, control-char-free string, positional, never a flag the operator did
  not declare) — a caller can never smuggle an extra argv element.
- **Activation (`POST /v1/relux/plugins/:id/command-tools`,
  `state.rs::configure_command_tool`).** Parsed by `command_tool_config.rs` (the same
  field-allowlist / derived-permission / clamp posture as `plugin_tool_config.rs`), it
  adds a manifest `ToolDefinition` (permission `tool:<plugin>:<verb>`, default risk
  **High**, approval **always Required** — a command tool is never auto-approved) and
  stores the `CommandToolConfig`. **Nothing runs at configure time.** The tool then
  surfaces in the Tools list as `needs_approval` (gated, not a dead end). Bundled and
  non-ToolSet plugins are refused; removal (`DELETE …/command-tools/:tool`) drops both
  the manifest tool and the recipe together.
- **Execution (`command_exec.rs`, only via `execute_tool_runtime`).** Invocation flows
  through the **existing** governed path — `POST /v1/relux/tools/invoke` (gated →
  `request-approval` → `execute`, or a standing allow-always grant). The executor
  re-validates the argv, resolves the `cwd` to canonicalize INSIDE the install dir
  (`secret_store::validate_managed_cwd` — a symlink escape is refused), spawns the
  command **argv-only, never a shell**, enforces the timeout (killing the child on
  expiry), captures stdout/stderr bounded to 64 KiB each and **secret-redacted**
  (`relux_core::redact_secrets`), and returns a shaped
  `{ exit_code, success, timed_out, stdout, stderr, …, duration_ms }`. A non-zero exit
  is a successful *run* with `success:false` (honest), a timeout is `timed_out:true`,
  and a missing required input / bad cwd / spawn failure is a clear, value-free error —
  never a fabricated success. Persistence: the recipe round-trips in the snapshot
  (`command_tool_configs`) and survives restart.

This is **MCP vs governed command tools** in one line: MCP exposes a *protocol* surface
(tools/resources/prompts) the kernel dials; a command tool runs *one local process* to
completion. Both are argv-only, both are permission-gated, approval-gated, and audited,
and neither is ever auto-run on import. Pinned by `relux_core::command_tool` unit tests
(argv contract, danger flag, traversal cwd, input-arg build/validation, timeout clamp),
`command_exec.rs` tests (live fixture run, non-zero exit, timeout kill, cwd-escape
refusal, missing-input fail-closed, output redaction), `state.rs`
`command_tool_configures_gates_runs_persists_and_removes` /
`a_disabled_command_tool_is_refused_not_run` /
`a_command_tool_on_an_uninstalled_plugin_is_refused`, and the server/dashboard tests
above.

#### Per-tool-call approval flow (gated tools)

A `needs_approval` tool can be run for ONE specific invocation through a real
per-call approval, without bypassing the gate (`docs/reference-driven-development.md`
"per-tool-call approval", borrowing openclaw's two-phase
`registerExecApprovalRequest` + consume-once `consumeExecApprovalFollowupRuntimeHandoff`):

1. **Request** — `POST /v1/relux/tools/request-approval { plugin_id, tool_name,
   input?, agent_id? }`. The kernel (`state.rs` `request_tool_invocation_approval`)
   validates the tool exists, the subject agent holds its permission, and the tool
   ACTUALLY requires approval (a directly-runnable tool is refused with
   `ToolDoesNotRequireApproval` — invoke it instead); it bounds the args to
   `MAX_TOOL_INVOCATION_ARGS_BYTES` (the loopback request cap), then creates a
   Pending `Approval` **and** a `PendingToolInvocation` binding to the exact
   `(plugin, tool, agent, args snapshot + SHA-256)`. Nothing runs. The needs_approval
   tool row offers this as an inline **Request approval** form on the Plugins page.
2. **Decide** — the Approvals page shows the request with its action, reason, risk,
   bound tool, and a **secret-redacted args preview** (`redact_args_for_preview`
   masks `token`/`password`/`secret`/`authorization`/… values; the raw snapshot is
   never shown). The operator Approves or Rejects (`/v1/relux/approvals/:id/decide`);
   a reject drops the binding outright.
3. **Execute once** — for an Approved, not-yet-consumed binding the operator clicks
   **Execute once** (`POST /v1/relux/approvals/:id/execute`). The kernel
   (`execute_approved_tool_invocation`) re-validates the tool still exists, the
   subject still holds the permission, and the stored args still hash to the recorded
   SHA-256, then runs the **stored snapshot** (never client-resupplied args, so the
   approved call cannot be modified) through the same loopback runtime as a direct
   invoke. The binding is **consumed on a single attempt** (success OR runtime
   failure) — it can never run again without a fresh approval. Every step is audited
   (`tool_invocation:request`/`execute`, success/denied/failed). The binding persists
   in the snapshot (meta-json seam, like `orchestrations`), so an approved call
   survives a restart.

This grants no blanket/reusable authority — one approval binds one invocation and is
consumed by one execution attempt; there is no `session`/`always` grant (the model
has no safe reusable-grant model, so none is invented). No remote/non-loopback
execution is added: the approved call runs through the same bounded loopback runtime,
so all existing safety bounds hold. Pinned by kernel tests
`per_call_approval_request_creates_a_bound_pending_approval`,
`per_call_approval_executes_once_after_approval_then_is_consumed`,
`a_runtime_failure_still_consumes_the_approved_invocation`,
`rejecting_a_per_call_approval_drops_the_binding`,
`requesting_approval_for_a_directly_runnable_tool_is_refused`,
`requesting_approval_without_the_permission_is_denied`,
`per_call_binding_survives_a_snapshot_roundtrip`,
`secret_args_are_redacted_in_preview_but_stored_verbatim`, and the dashboard
`toolReadiness` `canRequestApproval` assertions.

### Adapter Runtime v1 (local coding-agent CLIs)

An Adapter plugin (§8.1) decides how an assigned task runs. The bundled
`relux-adapter-local-prime` runs the deterministic echo path. Adapter Runtime v1
adds bundled adapters that drive a **local coding-agent CLI** the operator already
has installed, plus a generic command shape:

- `relux-adapter-claude-cli` &rarr; `claude -p --permission-mode default`
- `relux-adapter-codex-cli` &rarr; `codex exec`
- any other installed Adapter plugin &rarr; a generic command (requires an
  explicit binary).

Safety properties (the product safety bar, §17.5):

- **Disabled by default.** A CLI adapter never runs until the operator explicitly
  enables its runtime (CLI/API/dashboard). Relux never silently spawns a paid or
  interactive CLI.
- **No bypass.** Relux uses the Claude CLI's safe `--permission-mode default` and
  never passes `--dangerously-skip-permissions` or any danger/bypass flag.
- **argv only; prompt on stdin.** Commands are argv arrays (no shell string
  concatenation). The composed prompt (agent persona + task title/input) is fed on
  the child's stdin, so there is no arg-escaping surface and it works uniformly for
  native binaries and Windows `.cmd` shims.
- **Bounded + redacted.** Per-run wall-clock timeout (the child is killed on
  expiry), stdout/stderr byte cap, stderr capture, and obvious-secret redaction
  before output is persisted to the run transcript.
- **Permission/audit/run-event tracked.** Starting the run is permission-checked
  (`start_run`); the spawn, output, and every honest failure are written to the
  run transcript and the audit log.
- **Honest failures.** Disabled, unconfigured, missing binary, timeout, or
  non-zero exit marks the run AND task failed with the reason &mdash; never a
  fabricated success.
- **No secrets stored.** The per-adapter config persists only kind/command, the
  enabled flag, timeout, output cap, and an optional working dir.

Execution dispatch: `KernelState::execute_assigned_run` resolves the **effective**
adapter via `KernelState::effective_run_adapter` and dispatches on it. For a crew
operative with an explicitly-assigned CLI adapter that adapter is used verbatim; the
local-prime adapter runs the existing deterministic echo path; a recognized/enabled
CLI adapter spawns its local binary via `relux_kernel::adapter`; anything else fails
honestly. The Work page's "Run (Assigned)" action and the `task run-assigned` CLI
both route through this dispatcher.

**Brain-aware routing for a free-form Prime goal.** When the assigned adapter is the
local-prime default AND the task is a free-form natural-language goal with no gated
`tool_call`/`tool_plan` directive (`relux_core::is_unfulfillable_local_request`), the
dispatcher routes the run to the **real adapter Prime's configured brain resolves to**
(Claude/Codex CLI) rather than echoing on local-prime. The run record + transcript are
stamped with the adapter actually used (an `adapter_selected` event), so the Work page
shows the truth. If that CLI adapter is not enabled/on-PATH, the downstream CLI gate
fails closed with a clear setup action; if the resolved brain has no coding-agent run
adapter (`Local`/`Openrouter`, which is conversational only), the run stays on
local-prime and fails closed (below). The brain preference is a secret-free snapshot
the server re-syncs from the on-disk `AiConfig` before every run
(`KernelState::set_prime_brain_preference`); it carries no key material and changes no
durable-action path (every state change still flows through the deterministic kernel).

**Prime autonomy is unchanged**: the background autonomy tick still calls
`start_run` + `execute_local_run` directly (never `execute_assigned_run`), so it runs
only the deterministic local path and never auto-spawns a paid CLI (§17, "autonomy
does not call paid LLMs"). The brain-aware redirect is confined to the
operator-initiated `execute_assigned_run` chokepoint.

CLI:

```powershell
relux-kernel adapters
relux-kernel adapter runtime <adapter-id>
relux-kernel adapter runtime enable <adapter-id> [--timeout-seconds N] [--max-output-bytes N] [--command C] [--working-dir D]
relux-kernel adapter runtime disable <adapter-id>
```

API:

```text
GET    /v1/relux/adapters
GET    /v1/relux/adapters/:id/runtime
PUT    /v1/relux/adapters/:id/runtime    { "enabled", "command"?, "timeout_seconds"?, "max_output_bytes"?, "working_dir"? }
PATCH  /v1/relux/adapters/:id/runtime
DELETE /v1/relux/adapters/:id/runtime
```

Dashboard: the Crew page has an Adapters section with each adapter's honest status
(local / disabled / enabled-ready / enabled-but-binary-missing) and an
Enable/Disable control carrying the explicit note that Relux will run the local CLI
when an assigned task starts.

Adapters supported/detected in v1: `relux-adapter-claude-cli` (Claude CLI),
`relux-adapter-codex-cli` (Codex CLI), and a generic command adapter. Detection
probes `PATH` (and `PATHEXT` on Windows) read-only for the configured binary.

#### Local Prime is deterministic — it fails closed on real external work

The local Prime adapter performs **no external work**: no cloning a repository, no
filesystem/network access, no plugin import. It drives only the in-memory
control-plane loop (the deterministic echo, plus any operator-authored
`tool_call`/`tool_plan` directive that runs through the gated `call_tool`
chokepoint). A plain echo/test task keeps the echo behavior; a directive task runs
its gated tools.

But a task that the deterministic adapter cannot turn into actions — either a
**free-form natural-language goal** Prime was handed (a non-empty `prime_request`) with
**no executable directive**, OR a task whose **human title obviously denotes external
work** (clone a repo, import/install a plugin, download from a URL — the dashboard
"New Task" form shape, where the goal is the title and the input carries no
`prime_request`) — is outside it. Earlier such a run could be *started* and then sit in
`Running` forever with nothing executing it (the "running but nothing happens" bug) —
or fake-echo the input back as "done". The structural discriminator is
`relux_core::local_prime_cannot_fulfill(title, input)`, which combines
`is_unfulfillable_local_request` (the `prime_request` shape) with a narrow, conservative
title safety-rail `title_requires_external_execution` (per
`docs/reference-driven-development.md`, a keyword rail is a fallback safety net, never
the primary brain — it matches only unambiguous external-action phrasing so plain
echo/control-plane tasks are never caught). An operator-authored `tool_call`/`tool_plan`
directive always short-circuits to *fulfillable* (it runs through the gated tool path).

The first remedy is the **brain-aware routing** above: when the operator has a real
Prime brain configured (a Claude/Codex CLI adapter selected on Crew → Prime Brain),
`execute_assigned_run` routes the goal to that adapter and runs it for real. The
fail-closed branch below is the honest floor for when **no real brain is configured**
— so a free-form Prime run either uses a real brain or fails closed with a setup
action, never silently running on local-prime. When the resolved brain is `Local` or
`Openrouter` (conversational only, no coding-agent run adapter), the run stays on
local-prime and the **fail-closed branch** in `execute_local_run`
(`relux_core::local_prime_cannot_fulfill`) fires:

- the run reaches a terminal **`Failed`** state classified `adapter_missing` (never
  an infinite `Running`, never a fabricated success), and the task is parked
  **`Blocked`** (operator-actionable + reopenable once a real adapter is assigned);
- the run transcript + `run.error` carry **actionable guidance** naming the
  remedies: enable a **Claude/Codex** CLI adapter and select it as Prime's brain on
  **Crew → Prime Brain**, then run it again; or import a repository as a plugin via
  **Plugins → + Install → GitHub URL** (`KernelError::LocalAdapterUnsupported`, HTTP
  422). The guidance also notes that **OpenRouter is conversational only** and cannot
  execute work runs.

The two Prime-chat entry points that *create and run* / *start* a task
(`PrimeAction::CreateAndRunTask`, `PrimeAction::StartRun`) **drive the run to a
terminal state** through the same governed `execute_assigned_run` path the Work
page's "Run (Assigned)" uses — a started run is never left dangling. The HTTP
**`POST /v1/relux/tasks/:id/start`** route (behind the Work page's "Start" / "Run
(Assigned)" controls) likewise routes through `execute_assigned_run`, NOT a bare
`start_run`: a bare start would leave the run in `Running` with only a `run_started`
event and no executor behind it (the exact "running, No activity forever" symptom). A
fail-closed start returns **200** with the terminal run/task and a `refused` message
(mirroring `reopen-and-run`), so the card shows the honest outcome instead of a hung
card. The Work page's
recovery card recognizes the local-Prime case (an `adapter_missing` failure whose run
adapter is `relux-adapter-local-prime`) and offers **Set Prime's brain** (Crew →
Prime Brain) as the primary remedy, plus **Open Plugins** + **Reassign**, rather than
the generic "install your CLI" guidance. The run detail shows the human adapter label
+ an honest note that the local adapter does no external work.

### Bundled plugin refresh is idempotent (existing stores pick up new capabilities)

The shipped bundled plugins/adapters under `examples/relux-plugins`
(`relux-tools-echo`, `relux-tools-status`, `relux-adapter-local-prime`,
`relux-adapter-claude-cli`, `relux-adapter-codex-cli`) are reconciled into the
durable store on **every** load through a single central seam,
`relux_kernel::refresh_bundled_plugins` (called from `ensure_bootstrapped`). It is
no longer keyed on Prime's existence, so an existing local DB - not just a fresh
one - picks up newly shipped bundled plugins without a `reset-local`. Every CLI/API
path that ensures Prime/company also runs this refresh: `doctor`/`health`, `serve`,
`plugins`, `adapters`, `tools`, Prime/chat, and task execution.

The refresh is safe by construction (§9.4, §7.4):

- A bundled id that is not installed is added as a protected
  `PluginSourceKind::Bundled` record (enabled), and remains non-removable.
- A bundled id already installed as `Bundled` is updated **in place** only when the
  shipped manifest or its install metadata changed - no duplicate records - and the
  operator's `enabled` flag and per-plugin runtime config (HTTP loopback / CLI
  adapter) are preserved.
- An already-current store is a no-op: no re-registration, no audit noise.
- A plugin installed from a non-bundled source (a user install) that shares an id
  with a bundled plugin is **never** overwritten.

`relux-kernel doctor` and `relux-kernel plugins` refresh-and-save an older store on
the spot, so newly shipped bundled plugins appear without any manual reset.

### Optional LLM-backed Prime (OpenRouter)

As of Phase 2.1, Prime can optionally use an LLM (via OpenRouter) to shape its
conversational replies, making it feel more natural while remaining grounded
in kernel state.

- **Deterministic Fallback**: If no key is configured or `RELUX_LLM_DISABLED=1` is
  set, Prime remains fully deterministic.
- **Actionful Safety**: If a turn results in a state change (task creation, run
  start, etc.) or awaits approval, the reply remains deterministic to ensure
  absolute grounding. The LLM is never asked to narrate real state changes.
- **Conversational Shaping**: For greetings, status queries, and general chat, the
  LLM rephrases the kernel's grounded facts into natural dialogue.
- **Model picker (no slug memorization)**: OpenRouter model IDs are unintuitive and
  change, so the dashboard does **not** require typing a slug. Prime AI settings shows a
  searchable, selectable list of real models — name, context window, and prompt/completion
  price per million tokens — fetched live from OpenRouter's **public** catalog endpoint
  `GET https://openrouter.ai/api/v1/models`
  (official docs: <https://openrouter.ai/docs/api/api-reference/models/get-models>). The
  fetch is server-side, bounded (10s timeout, ≤4 MiB body, ≤400 models), and needs **no API
  key** (so no secret is exposed). It is exposed to the dashboard as
  `GET /v1/relux/ai/models` (session-protected), which **always** returns 200 with
  `{ ok, source, models, error? }`: on success `ok:true` + the reduced model list (id, name,
  context length, prompt/completion price, description, modality, in OpenRouter's server
  order — the dashboard floats the currently-configured model to the top); on failure
  `ok:false` + a short reason + empty list, so the UI degrades to an honest fallback (keep
  the manual slug field + a Retry button, never a blank picker). The catalog route is
  independent of `GET /v1/relux/ai/status` and never blocks it. The chosen model is saved
  through the **same** write-only secret-reference config path below — the picker never sees
  or stores a key. Manual slug entry remains as the advanced/fallback path.
- **Configuration (dashboard, recommended; no env vars)**: Prime Brain → OpenRouter →
  Prime AI settings lets an operator point Prime at an OpenRouter key/model without
  environment variables (§18: "do not hardcode one model provider"). The key is supplied
  **by reference**: its value lives write-only in the local secret store, and the AI
  config (`<data-root>/ai-config.json`, `0600` on Unix) stores **only the secret's name**
  (`api_key_secret`), never the value. It is resolved live by `serve`/CLI through the same
  `secret_store()` and is **never** returned by the API (see `docs/mcp.md` "Prime brain
  provider key by reference"). Endpoints:
  - `GET /v1/relux/ai/status` — key-free status (mode/configured/`secret_missing`/
    referenced `api_key_secret` name/model/reason).
  - `GET /v1/relux/ai/models` — the OpenRouter model catalog for the picker (public,
    key-free; always 200 with `{ ok, source, models, error? }`; honest fallback on failure).
  - `PUT /v1/relux/ai/config` — `{ provider:"openrouter", api_key_secret, model?,
    disabled? }` (the dashboard sends the secret NAME, not a key; the legacy plaintext
    `api_key` field is still accepted for env/CLI setups and is mutually exclusive with
    `api_key_secret`).
  - `DELETE /v1/relux/ai/config` — clear the stored config (reference + model).
  A referenced secret that is not set yields a clean `secret_missing` status (Prime stays
  deterministic, the reason names the missing secret) — no raw key ever leaks. Only
  OpenRouter takes a key; Claude/Codex adapters use their own local CLI login.
- **Configuration (environment, CLI-only setups)**:
  - `RELUX_OPENROUTER_API_KEY`: Enables OpenRouter when set.
  - `RELUX_OPENROUTER_MODEL`: Model ID (default `openai/gpt-4o-mini`).
  - `RELUX_LLM_DISABLED`: Forces deterministic mode even if a key exists.
  - `RELUX_LLM_TIMEOUT_MS`: Request timeout (default 15000ms).
  The dashboard secrets file wins per-field; omitted fields fall back to the env.

The API never returns the key. The dashboard shows the current AI provider/mode.

### MVP limitations (honest)

- The standalone Relux shell covers Home, Prime, Work, Crew, Plugins, Approvals,
  and Health — all backed only by the local `/v1/relux` control plane (no Relix
  web bridge, no login). These are the entire primary navigation. The old
  bridge-backed Relix pages (Command Center/Overview, Mandates, the Briefs board,
  Active Runs, the legacy Agents/Crew console, Chat, Settings, etc.) still exist in
  the bundle at their original paths for continuity, but they are NOT part of the
  standalone Relux shell and do not appear in its navigation. Visiting one directly
  shows the legacy Relix bridge console (clearly labelled legacy), which requires
  the old Relix web bridge + a login and degrades honestly when it is absent.
- Prime has an optional LLM-backed path for conversational replies, but its
  core planning remains deterministic. Multi-agent autonomous execution is later.
- The first-release **product path** is the Claude/Codex adapters + Prime tools.
  The built-in deterministic handlers (`relux-tools-echo`, `relux-tools-status`)
  are **internal dev/test tools** that prove the kernel/permission/audit loop and
  back the offline smoke; they are not the recommended user path and are not
  surfaced as a "run echo" affordance in the standalone shell.
- Tool invocation executes built-in deterministic handlers (echo, status) plus
  installed ToolSet plugins that an operator has explicitly pointed at a loopback
  HTTP server (Plugin Runtime v1). Relux still does NOT auto-run downloaded plugin
  code: it never shells out, never runs GitHub/zip/folder install code in-process,
  and never calls a remote host - a plugin becomes executable only via an
  operator-run `http://127.0.0.1|localhost|[::1]:<port>` endpoint.
- Installing a GitHub repo / folder / zip that has no `relux-plugin.json` no longer
  hard fails: Relux generates a safe, **metadata-only** wrapper manifest (sanitized
  id, no tools, no permissions, `Unverified`, marked generated). It runs nothing
  until the operator configures a runtime or adds tool definitions; Relux never
  infers tool commands from repo content. `/v1/relux/plugins` flags these with
  `generated: true`.
- Prime's AI provider/key is configurable from the dashboard (Health → Prime AI
  settings) without environment variables; the key is stored in a local gitignored
  secrets file and never returned by the API. Claude/Codex adapters use their own
  local CLI login (no key in Relux).
- Adapter Runtime v1 can drive an assigned task through a local coding-agent CLI
  (Claude CLI, Codex CLI, or a generic command), but only when the operator
  explicitly enables that adapter (disabled by default) and the binary is on PATH.
  Relux runs the CLI non-interactively with a safe, non-bypass permission mode and
  never passes `--dangerously-skip-permissions`. Each run records a durable,
  redacted, capped transcript (`run_started` → `adapter_spawn` → `adapter_output`
  → `run_completed`/`run_failed`), a **real measured** wall-clock `duration_ms`,
  and an honest pass/fail with a clear failure reason. When the CLI emits a
  structured result envelope (the Claude adapter requests `--output-format json`),
  Relux parses it into an honest text summary plus `usage`/`cost`, treats an
  envelope-reported `is_error` as a failure even on exit 0, and still stores the
  raw output; otherwise it surfaces the plain text honestly (Codex stays plain
  text). A **failed run is retryable** as a fresh run on the same task
  (`prime.retry_run`), with attempt lineage recorded (`retried_from`). It does
  **not** stream events live or resume a *partial* CLI run; a retry is a new
  attempt, not a resume. Execution-environment runtimes remain not implemented
  yet.
- **Local operator login v1.** The standalone dashboard/API now require a local
  sign-in (post-v0.1.4 auth slice). On first launch the dashboard shows a one-time
  **setup** form that creates a single local admin (username + **Argon2id** PHC
  password hash, stored at `dev-data/relux/dashboard-admin.json` next to the DB,
  gitignored, OS-restricted, **never** plaintext and never returned by the API).
  After setup, login mints an **HTTP-only** `relux_session` cookie
  (`SameSite=Lax`, `Path=/`; **no** `Secure` because the console runs
  over loopback `http://` — a TLS-terminating reverse proxy can re-add it). The
  serve auth middleware protects every `/v1/relux/*` route behind a valid session;
  the dashboard `fetch` rides the cookie automatically (no token paste).
  Sessions are **sliding/rolling**: the 12h window is an **idle timeout**, not a
  fixed lifetime. On every *successful* protected request the middleware slides
  the session's idle deadline forward by 12h and re-emits the cookie with a fresh
  `Max-Age`, so an actively-used console never expires out from under the
  operator, while one left idle for 12h still times out. A **hard absolute
  ceiling of 7 days** (`SESSION_ABSOLUTE_MAX_SECS`) caps the rolling renewal: a
  session can never be slid past 7 days from when it was minted, so a continuously
  active (or stolen) cookie is forced to re-authenticate after a week. The refresh
  is attached **only** on an authenticated request that returns a success status —
  a 401 from the guard, or a 4xx/5xx from the handler, never carries a session
  cookie. Status polls (`/v1/auth/status`, `/v1/auth/me`) validate **without**
  sliding, so background polling alone does not keep a session alive. To make the
  rolling policy *visible*, `/v1/auth/me` returns safe, secret-free **session
  metadata** alongside the username: the idle and absolute deadlines
  (`idle_expires_at` / `absolute_expires_at`), the seconds remaining on each
  (`idle_expires_in_secs` / `absolute_expires_in_secs`, clamped ≥0), the configured
  policy windows (`idle_timeout_secs` / `absolute_max_secs`), and the server clock
  (`server_now`). It **never** exposes the session id, the cookie value, or the
  admin hash. Because the read is non-sliding, the deadlines are the **current,
  pre-refresh** values (what the cookie reflects now — not a window bumped by the
  read). The dashboard **Account** control renders this as *"Signs out after 12h of
  inactivity"* / *"Re-sign-in required after 7d"* with a live, locally-counted
  *"… left"* readout (a single per-minute timer; under `RELUX_AUTH_DISABLED` it
  shows an honest *"Session expiry is disabled"* note instead). The shell also
  surfaces a **passive, low-noise expiry chip** in the topbar that appears only
  when a deadline is close — amber for the rolling idle window (≤10 min left,
  since any action slides it forward) and red for the hard absolute ceiling (≤30
  min left, warned earlier because only a fresh sign-in clears it; on a tie the
  absolute warning wins). Clicking it opens the Account control. The chip reads
  the SAME non-sliding `/v1/auth/me` metadata sparsely — once on shell mount, then
  re-anchored only on event-driven moments (the tab regaining visibility, the
  Account panel closing) — never a busy poll, which would be pointless since the
  read does not slide the session; a single per-minute timer counts down locally
  between fetches, and the chip stays hidden under `RELUX_AUTH_DISABLED` or for an
  older kernel that omits the deadlines. Because the hard absolute ceiling cannot
  be slid by anything the operator does in the console, the **Account** control
  pairs the readout with a clear **re-authentication path**: a *"Sign out and sign
  back in"* button that ends the current session (via the existing
  `POST /v1/auth/logout`) so the normal sign-in screen reappears and a fresh login
  mints a new session — the only thing that resets the 7-day cap. It **never**
  auto-submits credentials (the operator still types their password on the login
  screen) and never weakens auth. The button is always present in Account, and is
  **emphasised** — promoted to the primary action with an alert banner — exactly
  when the absolute ceiling is inside its warning window (the same ≤30 min the red
  chip uses); when the ceiling is comfortably far off it stays a quiet secondary
  control. Signing out this way leaves other sessions untouched, and the
  password-change form is unchanged (a failed sign-out keeps the session intact and
  surfaces the reason, with the topbar **Sign out** control as the fallback). Public by
  design: the static dashboard (so the setup/login screen always renders — never a
  blank page), the public auth endpoints (`/v1/auth/status`/`setup`/`login`/
  `logout`/`me`), and `/v1/relux/health` (liveness probe). `POST
  /v1/auth/change-password` is the one auth endpoint that is **protected** (it
  requires a session, so it sits behind the same guard as `/v1/relux/*`). Sessions are
  **restart-persistent**: the session table is mirrored to a gitignored local file
  (`dev-data/relux/dashboard-sessions.json`; `RELUX_SESSION_FILE` overrides) next to the
  admin credential, storing a **SHA-256 hash of the sid** plus its deadlines (never the
  raw cookie value), so a restart reloads still-live sessions instead of re-prompting,
  while the admin credential stays durable. Expired rows are pruned on load and on use. A signed-in operator can **change the password in-product** via
  `POST /v1/auth/change-password` (`{ current_password, new_password }`, behind the
  session guard; surfaced by the dashboard's **Account** control): it verifies the
  current password, enforces the same 8-char minimum, and rewrites the credential
  with a fresh Argon2id hash through the same atomic write as setup — never logging
  or returning the plaintext/hash. A successful change **preserves the caller's own
  session and invalidates every OTHER live session** (a change boots other
  browsers/devices but not the current tab). Password recovery when the current
  password is *unknown* stays the **local** `relux-kernel reset-admin`
  CLI (filesystem-only, no network/unauthenticated reset; it also clears the persisted
  session file, and a **running** `serve` reconciles against that deleted file and
  drops its in-memory sessions on the next request — so old cookies stop working
  without a restart). A dev/test-only escape hatch `RELUX_AUTH_DISABLED` leaves the API
  open (OFF by default, flagged loudly by `serve` and `doctor`). The CLI
  (`prime`, `task run-assigned`, `tools`, autonomy, …) talks to the durable store
  directly and is unaffected by HTTP auth.
- The standalone API is local-first and binds **loopback only**; it is now gated
  by the single-admin local operator login above (not the earlier
  "unauthenticated by design"). It remains a single-operator local console, not a
  multi-tenant or internet production surface: one admin account, locally-persisted
  sessions (a hash of the sid + deadlines, gitignored — surviving a restart), and a
  loopback bind with no transport TLS (http).

### Status after v0.1.1 — next unfinished pieces

As of **v0.1.1** the local loop is usable end-to-end without developer knowledge:
boot the bundle, pick a Prime brain (Local / Claude CLI / Codex CLI / OpenRouter)
from Health, chat with Prime, and create/assign/run tasks. The honest gaps that
remain, in rough priority for the next slices:

1. **First-run onboarding.** *(Largely addressed post-v0.1.1.)* Home's first-run
   checklist now derives a **live "connect Prime to a brain" step** from the
   control plane (`/v1/relux/ai/status` + `/v1/relux/adapters`): it detects whether
   the Claude/Codex CLI is on PATH, reports whether the selected brain is actually
   usable, and gives the exact next step — always routed to Health → *Prime Brain /
   AI Runtime*, never the legacy Crew path. The pure derivation lives in
   `apps/dashboard/src/onboarding.ts` with unit coverage in
   `apps/dashboard/test/onboarding.test.ts`. *(Extended by the "first-run guided
   launchpad" slice — see the changelog entry above §22.)* Home's checklist now
   carries the **full guided journey** end-to-end: brain → adapter → crew →
   plugins/tools → **try Prime (the first useful turn)** → **resume paused work** →
   **inspect stuck work**, each a pure `readiness.ts` derivation with a single
   honest first action. A fuller modal walkthrough (a separate guided overlay) is
   still optional polish, but the in-page launchpad now guides every step.
2. **Plugin install UX.** *(Addressed post-v0.1.1.)* See "Plugin Install UX v1"
   below. A generated metadata-only wrapper is now badged **Needs configuration**
   (never "enabled"/"ready"); its honest next step is **add tool definitions** (a
   `relux-plugin.json`), not "configure a runtime" — because a wrapper declares no
   tools, a loopback runtime alone surfaces nothing. The Plugins page gives a
   one-click **Set up** affordance with a copy/download manifest template, the
   install flow shows a **result summary** (tools discovered vs wrapper generated
   vs adapter, plus the next step), and the Tools list shows **only runnable tools
   by default** with a toggle for the rest. The pure derivations live in
   `apps/dashboard/src/plugins.ts` with unit coverage in
   `apps/dashboard/test/plugins.test.ts`.
3. **Adapter run depth.** *(Addressed post-v0.1.1.)* A CLI adapter run is now
   observable, understandable, and recoverable: the Work page's Run Detail shows
   the adapter, status, current/last **phase**, a **real measured duration**, a
   redacted **output excerpt**, a clear **failure reason**, and (when the CLI
   reported them) **cost/usage** — all read from the durable transcript, never
   fabricated. Run Detail is **URL-addressable in-shell**: `/work?run=<run_id>`
   opens that run's panel (the param is the source of truth, so deep links and
   browser back/forward/refresh restore it, and a missing run degrades to an honest
   notice). An orchestration step's `run_id` deep-links here via `workRunHref`,
   keeping the operator on the Relux Work surface rather than the legacy `/runs`
   console. Every in-shell run reference resolves to this one surface: a run's
   `retried_from` lineage (Recent Runs + the Run Detail "Retry of" field) is a
   `/work?run=` link to the parent in the same Relux ledger, and the Run Detail
   header carries a **Copy link** button that copies the absolute
   `<origin>/dashboard/work?run=<id>` URL (`workRunShareUrl`, clipboard-with-inline
   fallback). The legacy `/runs` console is a *separate* ledger (`/v1/runs`,
   `brief_runs`) whose ids do not exist on relux-kernel, so its links stay on
   `/runs` and are deliberately **not** rewritten to `/work?run=`. The Claude adapter requests a JSON result envelope that the kernel
   parses into an honest summary + metrics (`relux_core::parse_adapter_result`),
   and an envelope `is_error` is treated as a failure even on a clean exit; Codex
   and generic commands degrade honestly to plain text. A **failed run is
   retryable** from the UI/API/CLI as a fresh run on the same task
   (`prime.retry_run` → `POST /v1/relux/runs/:id/retry`), with lineage recorded
   (`retried_from`); the HTTP path persists a failed run so its transcript
   survives a refresh. Run Detail also ports the run-depth **tool calls** field
   (§11.3) — derived honestly from the durable transcript (`toolCallSummary`
   counts real `tool_call` / `tool_call_denied` / `tool_call_failed` events, never
   inferred). **First real Relux run artifact model (read-only capture):** when an
   adapter's structured result envelope declares `artifacts: [...]`, the kernel
   now captures those as bounded, redacted, **path-sanitized references**
   (`relux_core::capture_run_artifacts` → `Run.artifacts`, persisted on the
   durable run record so a refresh shows them) and `GET /v1/relux/runs/:id`
   flattens them onto the detail; the Work Run Detail lists them read-only
   (name / type / summary / source, `runArtifacts`/`artifactTypeLabel`) with an
   honest empty state. Safety: the count and every field are capped, secrets are
   redacted, and an unsafe declared path (absolute / drive / UNC / `..`) is dropped
   — the kernel never reads the underlying file. The references are **capture only** and
   NOT the legacy `/runs` workspace changed-file set — the two share no ids or
   store. **First real Relux diff/apply model (reviewed proposed changes):** an
   adapter envelope may ALSO declare `proposed_changes: [...]` — reviewable,
   applyable **full-content file replacements** (each `path` / `new_content` /
   `baseline_sha256` / computed `new_sha256` / `bytes` / `source` / `status`),
   captured by `relux_core::capture_proposed_changes` → `Run.proposed_changes`
   (persisted; survives a snapshot round-trip) and flattened onto
   `GET /v1/relux/runs/:id`. The operator **reviews** each
   (`POST …/proposed-changes/:index/review` → approve / reject) and, once approved,
   **explicitly applies** it (`POST …/proposed-changes/:index/apply`) into the run's
   **controlled workspace root** (the adapter's `working_dir`). **Nothing is ever
   auto-applied.** Apply (the one place the kernel writes an agent-proposed file)
   refuses honestly and never fabricates success: it requires `Approved` state,
   **refuses without a baseline hash** (no force in v1), requires a configured
   workspace root, resolves the target **inside** that root with no `..`/symlink
   escape, refuses excluded (vcs/build/secret) paths, requires an **existing
   regular file whose current SHA-256 equals the baseline** (a mismatch is an
   honest **conflict**, the file untouched), and writes **atomically**. Capture is
   bounded (`MAX_PROPOSED_CHANGES = 32`, `MAX_CONTENT_BYTES = 256 KiB`, text-only,
   path-sanitized). The Work Run Detail surfaces a **Proposed Changes** section
   (per-change status, content preview, approve / reject / apply, honest refused
   reasons / conflicts), and `reviewApplyAvailability` now returns
   `available:true` when a run proposed changes (apply is real for them); a run
   with only read-only references keeps the honest "no diff/apply" reason.
   **Proposed changes are captured ONLY on the assigned-run execution path.** The
   Prime *conversational brain* path (Claude/Codex CLI answering a chat turn) is
   **action-free by design** — it only runs on non-actionful turns
   (`is_actionful`), the chat prompt forbids claiming any state change, and
   `run_cli_brain` "only ever shapes a conversational reply; it never performs a
   durable action". So even if a chat-turn envelope declares `proposed_changes`,
   the kernel does **not** capture them into a run: there is no chat-turn run to
   hang review/apply on, and synthesizing one would manufacture hidden, mutable
   work from a casual message. The chat bubble still shows only the human `result`
   text (`shape_cli_brain_reply`, never the raw JSON), and rather than drop the
   change silently the kernel surfaces a bounded, secret-free **advisory note**
   (`brain_envelope_advisory`) telling the operator a change was proposed and to
   create a task assigned to that adapter and run it — the documented path that
   captures proposed changes with the safe review/apply flow. Structurally, the
   Prime chat response wire (`PrimeTurn`) carries no `proposed_changes`/`artifacts`
   field, so a proposed change can never reach the chat surface. **Apply now
   supports four actions** (`action: "replace"` — the default and historical
   behavior — `action: "create"`, `action: "rename"`/`"move"`, or
   `action: "delete"`/`"remove"`; a missing action defaults to `replace` for
   backward compatibility): a `replace` is a full-content
   replacement over an existing baseline file (a missing target is a conflict); a
   `create` adds a **new file** that must NOT already exist (an existing path is a
   conflict — never overwritten), carries **no baseline**, and creates any missing
   parent directories (each a sanitized, non-excluded, in-root component, with no
   symlink crossing) before placing the file atomically (an O_EXCL reservation +
   temp + rename, so a racing creator never clobbers); a `rename` **moves** an
   existing baseline file from `path` to a new `dest_path` (both sanitized + root-
   confined), preserving its content (so it carries **no new content**) — it
   verifies the **source still matches its baseline** (a mismatch is a conflict),
   refuses if the **destination already exists** (a conflict) or equals the source,
   creates any missing destination parent dirs, then moves the file; and a `delete`
   **removes** an existing baseline file at `path` (carrying **no content** and **no
   destination**) — it verifies the target is an **existing regular file** (never a
   directory or symlink) that **still matches its baseline** (a mismatch is a
   conflict), then removes it. All four
   actions share the same approval gate, path/exclusion checks, workspace-root
   confinement, transactional set-apply (validate-all-then-write-all, with creates
   rolled back by deletion, renames moved back to their source, and deletes
   recreated from their captured bytes on a mid-apply fault; a rename occupies BOTH
   its source and destination, and a delete occupies its target, so no two changes
   may overlap a path), and honest 409/422 refusals. What is still **not** done:
   arbitrary patch/diff parsing
   (deliberately not built — replacement is safer); live event streaming (the page
   polls/refreshes a synchronous run rather than tailing it); and resuming a
   *partial* CLI run (retry is a new attempt). Execution-environment runtimes are
   not implemented.
4. **Multi-agent autonomy.** *(First slice addressed post-v0.1.2; depth slice
   added after.)* See "Orchestration (First Multi-Agent Slice)" below. Prime can
   decompose a multi-step goal into role-typed **briefs assigned to different
   agents** and run them in a **governed, dependency-aware, round-based batch**
   through each agent's own adapter (local Prime echoes; an enabled Claude/Codex CLI
   agent runs the real CLI), recording per-agent outcomes and a durable goal →
   brief → agent → run trace. The planner now **infers simple dependencies**
   (implementation waits on research; testing/review/documentation wait on
   implementation), and the run loop **gates on them** — running only ready briefs,
   honestly blocking a brief whose dependency failed, and grouping independent
   ready briefs into **rounds bounded by a concurrency cap** (default 2, clamp
   1..=4), with per-brief start/finish/round recorded for progress. A round's
   independent ready briefs run as **true OS-parallel adapter processes** on **every
   path** — the non-blocking job path (`run-async`), the **synchronous** `POST …/run`,
   and the `prime orchestration run` CLI all drive one **shared executor** split into
   prepare / spawn (one OS thread per brief) / finalize phases, so up to the cap run
   at once while every governance check, the transcript, audit, and retry stay under
   the lock and no downloaded plugin code is auto-run. The job path releases the lock
   around the spawn window and persists between rounds for responsive polling; the
   synchronous API/CLI hold the kernel for the batch (blocking until done) so two
   concurrent runs can never double-execute a brief. What is still **not** done:
   **live mid-run progress streaming** on the synchronous path (the job path does poll
   real in-flight briefs); automatic
   agent hiring during planning (Prime falls back to itself
   and suggests a hire); and a background timer that drives orchestrations (running
   is operator-triggered from the UI/CLI/API; the background autonomy timer stays
   deterministic and never spawns a paid CLI).
