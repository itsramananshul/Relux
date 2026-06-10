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
  GET /v1/relux/runs/:id
  GET /v1/relux/runs/:id/events
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
- `RELUX_HTTP_ADDR` - the bind address (default `127.0.0.1:19891`).
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
builds the release binary, runs `doctor`, and smoke-tests Prime task creation
plus assigned-task execution against a temporary `RELUX_DB`. `-SkipSmoke` skips
that quick Prime smoke; `-FullE2E` additionally runs `scripts\relux-e2e-smoke.ps1`
(it reuses the just-built release binary).

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
command + disable, **never spawning real Claude/Codex**); the autonomy loop (a
ready task created through Prime moves Queued -> Completed via one tick); and the
`serve` HTTP endpoints (`/dashboard`, `/v1/relux/state`, `/v1/relux/prime/autonomy`,
`/v1/relux/tools`). Flags: `-SkipBuild`, `-SkipServe`, `-SkipLoopback`, `-KeepTemp`.
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
  missing.
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
- The standalone API binds loopback and is unauthenticated by design - it is not
  a multi-user or production surface.
- GitHub Actions stay disabled; releases are cut by hand with this script.

### Release history (local Windows bundles)

Relux ships as hand-cut, local-first Windows bundles (no installer, no hosted
download). The version is the `relux-kernel` / `relux-core` crate version and is
stamped into `relux-kernel doctor`, `/v1/relux/health`, and the bundle's
`VERSION.txt`. Build a bundle with `scripts\relux-package-local.ps1 -FullE2E`.

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
  real agent on the live roster (by id keyword) or `None` (→ Prime fallback, with
  an honest hire note). It is conservative: a goal that does not split into ≥2
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
- **Honest concurrency note:** `concurrency` bounds the *round size* and pins the
  scheduler contract; briefs within a round currently execute **sequentially**
  through the kernel's single-owner lock (no OS-parallel CLI spawns yet). Real
  OS-parallel execution within a round is a later slice.

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
GET  /v1/relux/orchestration-jobs/:job_id     # poll one job: state queued/running/completed/failed + round/step statuses/result
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
active so a second click can't start a duplicate (the backend also rejects it). On
completion the panel folds the job's aggregate result into the "Last batch" banner
and refreshes the durable record. Home shows the newest unfinished orchestration
with its progress and next action. Pure UI logic lives in
`apps/dashboard/src/orchestration.ts` (job helpers: `jobIsActive` / `jobIsTerminal`
/ `jobPhaseLabel` / `jobProgressLabel` / `jobRunningStepIds` / `runButtonLabel`)
with unit coverage in `apps/dashboard/test/orchestration.test.ts`.

Progress visibility is now honestly **live**: a `run-async` job runs on a
background thread that drives the SAME governed, tested `run_orchestration` one
round at a time — releasing the single-owner kernel lock and persisting the
orchestration record **between** rounds — so polling the job (or the durable
record) sees real, already-recorded per-brief start/finish/round and the
dependency-aware ready/waiting/blocked state **as the batch progresses**, not only
after it returns. The blocking `/run` endpoint stays for the CLI/tests. Two honesty
contracts hold: (1) the briefs about to run this round are reported as `running`
from the durable readiness rule — nothing fabricates in-flight progress; (2) the
job registry is **in-memory only**, so a server restart mid-job loses the job
record (a poll then 404s and the dashboard falls back to the durable record, which
still carries whatever rounds actually completed). The worker never spins: each
round moves ≥1 brief to a terminal outcome and it stops as soon as a round runs no
brief, the per-job budget is spent, or the orchestration is no longer `running`.
Duplicate starts are rejected (409, one active job per orchestration) and the fleet
is capped (429 past `MAX_ACTIVE_JOBS`). Backend job lifecycle/duplicate/cap/aggregate
logic is unit-tested in `crates/relux-kernel/src/server.rs`; an end-to-end HTTP
smoke (`scripts/smoke-orchestration-job.ps1`, plus a real-Claude-CLI variant
`scripts/smoke-orchestration-job-claude.ps1`) proves the start → poll → terminal
path against a live kernel.

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

Installing a GitHub repo / folder / zip with no `relux-plugin.json` succeeds as a
**generated metadata-only wrapper** (sanitized id, zero tools, zero permissions,
`Unverified`, `generated: true`). The dashboard now makes that state honest and
actionable instead of leaving the operator to wonder:

- **No "ready"-looking label.** A wrapper is badged **Needs configuration** (amber),
  never the green "enabled" a real plugin shows. Its row carries an inline banner
  stating the dead-end plainly: it declares no tools, so a runtime alone runs
  nothing.
- **The honest next step is a manifest, not a runtime.** Because `discover_tools`
  only surfaces manifest-declared tools, a wrapper with no tool definitions stays
  empty even with an enabled loopback runtime (pinned by the kernel test
  `enabling_a_runtime_on_a_wrapper_surfaces_no_tools`). So the wrapper's call to
  action is **Set up → add tool definitions**, not "configure a runtime". The Set
  up panel hands the operator a ready-to-edit `relux-plugin.json` (copy or
  download), keyed to the plugin's id, plus the exact install directory, and the
  three-step path: add the manifest → re-install (Local folder) → point a loopback
  runtime at a local server. Relux still never infers tools from repo content and
  never runs downloaded code.
- **Plugin categories are distinct.** The Kind column distinguishes **Adapter**
  (configured on the Crew page), **ToolSet** (with its declared tool count and a
  loopback **Runtime** panel), **Metadata-only wrapper** (Set up → manifest), and
  internal dev/test plugins (hidden by default, §echo). A real manifest-based
  plugin is unaffected and keeps its normal Runtime flow.
- **Install result summary.** After an install the panel stays open and reports
  what happened — tools discovered (count), a wrapper generated (nothing runnable
  yet), or an adapter installed — and the exact next step, instead of a bare
  "Installed X".
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

Execution dispatch: `KernelState::execute_assigned_run` resolves the assigned
agent's adapter. The local-prime adapter runs the existing deterministic echo
path; a recognized/enabled CLI adapter spawns its local binary via
`relux_kernel::adapter`; anything else fails honestly. The Work page's
"Run (Assigned)" action and the `task run-assigned` CLI both route through this
dispatcher. **Prime autonomy is unchanged**: it still runs only the deterministic
local path and never spawns a CLI (§17, "autonomy does not call paid LLMs").

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
- **Configuration (dashboard, recommended; no env vars)**: Health → Prime AI
  settings lets an operator set the OpenRouter key/model without environment
  variables (§18: "do not hardcode one model provider"). The key is stored in a
  local, gitignored secrets file under the data root (`<data-root>/ai-config.json`,
  `0600` on Unix), is resolved live by `serve`/CLI, and is **never** returned by the
  API. Endpoints:
  - `GET /v1/relux/ai/status` — key-free status (mode/configured/model/reason).
  - `PUT /v1/relux/ai/config` — `{ provider:"openrouter", api_key, model?, disabled? }`.
  - `DELETE /v1/relux/ai/config` — clear the stored key/config.
  Only OpenRouter takes a key; Claude/Codex adapters use their own local CLI login.
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
- The standalone API is local-only and unauthenticated by design; it binds
  loopback. It is not a multi-user or production surface.

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
   `apps/dashboard/test/onboarding.test.ts`. A fuller modal walkthrough (a single
   guided flow that ends on a first chat/task) is still optional polish.
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
   fabricated. The Claude adapter requests a JSON result envelope that the kernel
   parses into an honest summary + metrics (`relux_core::parse_adapter_result`),
   and an envelope `is_error` is treated as a failure even on a clean exit; Codex
   and generic commands degrade honestly to plain text. A **failed run is
   retryable** from the UI/API/CLI as a fresh run on the same task
   (`prime.retry_run` → `POST /v1/relux/runs/:id/retry`), with lineage recorded
   (`retried_from`); the HTTP path persists a failed run so its transcript
   survives a refresh. What is still **not** done: live event streaming (the page
   polls/refreshes a synchronous run rather than tailing it) and resuming a
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
   1..=4), with per-brief start/finish/round recorded for progress. What is still
   **not** done: **OS-parallel** execution *within* a round (briefs in a round
   still run sequentially through the kernel's single-owner lock — the cap bounds
   the round size and pins the contract, but does not yet spawn CLIs concurrently);
   **live mid-run progress streaming** (an HTTP run is synchronous; the dashboard
   renders the recorded round/timing/dependency state after the batch returns, not
   a live feed); automatic agent hiring during planning (Prime falls back to itself
   and suggests a hire); and a background timer that drives orchestrations (running
   is operator-triggered from the UI/CLI/API; the background autonomy timer stays
   deterministic and never spawns a paid CLI).
