# Relux Detailed Product and Technical Specification

## Version

Version: 0.3.0
Status: Draft
Architecture Direction: Plugin-First Control Plane
Primary Category: Control Plane for Agentic Applications
Secondary Category: Plugin Kernel for Agentic Software Infrastructure

---

# 1. Executive Summary

Relux is a plugin-first control plane for building, managing, securing, and scaling agentic applications.

Relux is not another chatbot, not another single agent, and not just another workflow builder. Relux is the operating layer that lets developers and companies assemble complete agentic systems out of reusable components.

The core idea is simple:

Everything is a plugin.

Agents are plugins.
Tools are plugins.
Models are plugins.
Databases are plugins.
Memory systems are plugins.
Vector stores are plugins.
Execution environments are plugins.
Task brokers are plugins.
Company integrations are plugins.
Custom internal systems are plugins.

Relux gives all of these plugins one shared place to be discovered, installed, configured, permissioned, routed, audited, and orchestrated.

Instead of every developer rebuilding authentication, permissions, storage, memory, integrations, task routing, execution environments, logging, and dashboards from scratch for every new AI application, Relux provides a lightweight control plane where those pieces can be plugged in and reused.

The experience should feel like:

```text
Need OpenAI? Install an adapter plugin.
Need Claude? Install an adapter plugin.
Need GitHub tools? Install a ToolSet plugin.
Need Postgres? Install a ServiceProvider plugin.
Need Qdrant? Install a vector store plugin.
Need Python execution? Install an ExecutionEnvironment plugin.
Need SOL execution? Install the SOL runtime plugin.
Need browser automation? Install a browser tools plugin.
Need internal company CRM access? Build a private plugin.
Need an agent that can only create PRs but not merge them? Assign scoped permissions.
```

Relux turns agentic development from custom glue work into a reusable, permissioned, observable, and extensible ecosystem.

The long-term goal is for Relux to become the control plane for the agentic software stack.

---

# 2. Product Vision

## 2.1 The Big Vision

Relux exists because the future of software will not be built around one model, one agent, one tool, or one closed platform. It will be built around networks of agents, tools, systems, users, permissions, and company data that all need to work together safely.

Today, most AI systems are isolated. One agent has one set of tools. Another agent has another memory system. One app uses one authentication layer. Another app uses another database. One tool is written in Python. Another is written in TypeScript. One workflow runs locally. Another runs in the cloud. Every time a developer wants to connect these systems, they have to write glue code.

Relux creates a shared control layer where all of these pieces can connect.

A company using Relux should be able to say:

```text
These are our agents.
These are our tools.
These are our users.
These are our plugins.
These are our permissions.
These are our workflows.
These are our runtimes.
These are our databases.
These are our internal systems.
This is what each agent can do.
This is what each agent cannot do.
This is what happened.
This is what failed.
This is what needs approval.
```

Relux should make agentic systems feel manageable.

## 2.2 What Relux Is

Relux is:

1. A plugin-first control plane for agentic applications.
2. A lightweight kernel that loads and orchestrates plugins.
3. A permission system for agents, tools, users, and actions.
4. A routing layer between agents, tools, models, and infrastructure.
5. A dashboard for managing agents, plugins, permissions, tasks, logs, and system health.
6. A developer platform for building reusable agentic systems.
7. A company-level operating layer for AI-powered internal infrastructure.
8. A framework for turning disconnected tools into one connected ecosystem.

## 2.3 What Relux Is Not

Relux is not only an AI agent.

Relux can run agents, but Relux itself is the control plane around them.

Relux is not only a chatbot UI.

Chat can be one interface, but Relux should also support APIs, dashboards, workflows, task queues, CLI usage, and system-to-system execution.

Relux is not only a workflow builder.

Workflows are part of the system, but Relux is broader. It manages agents, plugins, permissions, tools, execution, storage, and tasks.

Relux is not tied to one model provider.

OpenAI, Anthropic, Ollama, Gemini, Grok, OpenRouter, local models, and future models should all be pluggable.

Relux is not tied to one database.

SQLite, Postgres, Qdrant, ChromaDB, Redis, NATS, S3, and other storage systems should be replaceable through provider plugins.

Relux is not a closed platform.

The entire design should allow official plugins, community plugins, private company plugins, and local plugins.

---

# 3. Core Positioning

## 3.1 One-Line Positioning

Relux is the plugin-first control plane for building secure, interoperable, and scalable agentic applications.

## 3.2 Stronger Positioning

Relux is the control plane for the agentic software stack. It lets builders plug together agents, tools, models, memory, databases, runtimes, workflows, and company systems through one permissioned, observable, and extensible layer.

## 3.3 Developer-Focused Positioning

Relux lets developers build agentic applications without rebuilding auth, storage, memory, tool routing, permissions, execution environments, and integrations from scratch every time.

## 3.4 Company-Focused Positioning

Relux lets companies safely deploy and manage AI agents across internal tools, systems, users, permissions, workflows, and data.

## 3.5 Ecosystem-Focused Positioning

Relux turns agents, tools, models, and infrastructure into installable plugins inside one connected ecosystem.

---

# 4. The Problem

## 4.1 Developers Rebuild the Same Foundation Repeatedly

Every new AI application usually needs the same infrastructure:

Authentication.
User management.
Role-based access.
Tool permissions.
Model routing.
Task execution.
Logs.
Memory.
Vector search.
Database storage.
Secrets.
Workflow state.
Error handling.
Dashboard UI.
Tool calling.
Agent configuration.
Deployment configuration.
Integration setup.

Most developers do not want to spend weeks rebuilding this. They want to build the actual product.

Relux solves this by making the foundation reusable.

## 4.2 Agentic Applications Are Too Hard to Connect

Modern agentic applications depend on many moving parts.

A coding agent may need:

GitHub.
Terminal.
Browser automation.
Python execution.
A model provider.
A memory store.
A database.
A deployment environment.
A task queue.
A permission system.

A research agent may need:

Web search.
PDF parsing.
Browser access.
Internal document access.
Citation tools.
Memory.
Company knowledge base.

A support agent may need:

Zendesk.
Slack.
Customer database.
Refund policy tool.
Email.
Approval flow.
Audit logs.

Without Relux, every connection becomes custom glue code.

Relux turns these capabilities into plugins.

## 4.3 Systems Do Not Speak the Same Language

One tool may be written in Python.
Another may be written in TypeScript.
Another may expose REST.
Another may expose gRPC.
Another may run locally.
Another may run inside Docker.
Another may be a company-internal API.

The developer has to make all of them talk.

Relux provides a shared interface where plugins register capabilities, schemas, permissions, and execution methods.

## 4.4 Agents Are Powerful But Unsafe Without Permissions

Agents should not automatically have access to everything.

A coding agent should not automatically be able to delete production databases.
A GitHub agent should not automatically be able to merge into main.
A support agent should not automatically be able to issue refunds.
A research agent should not automatically access private company records.
A terminal agent should not automatically run destructive commands.

Relux solves this by treating every action as permissioned.

Agents only get the capabilities explicitly granted to them.

## 4.5 Current Agent Systems Are Fragile

If one part of an agentic system breaks, the whole system can fail.

For example:

A GitHub tool fails, so the coding workflow dies.
A model provider is down, so the entire app stops.
A memory store fails, so unrelated tools also fail.
One agent crashes, so the whole task is lost.

Relux should isolate failures.

A broken plugin should not kill the kernel.
A failed agent should not kill the whole system.
A failed tool call should be retryable, replaceable, or escalated.
A disabled plugin should not corrupt global state.

Relux should be modular enough that parts can fail independently.

## 4.6 Companies Need Manageability, Not Just Agents

Companies do not only need agents. They need to manage agents.

They need to know:

Which agents exist?
Who created them?
What plugins power them?
What tools can they use?
What data can they access?
What tasks are they running?
What failed?
What succeeded?
What needs approval?
What actions were taken?
Who approved them?
What secrets are connected?
What plugins are unhealthy?
What version is running?

Relux should provide this operator view.

---

# 5. Core Product Principles

## 5.1 Everything Is a Plugin

The kernel should remain small. Most functionality should be added through plugins.

This includes:

Model providers.
Agent adapters.
Toolsets.
Databases.
Memory stores.
Vector stores.
Execution environments.
Task brokers.
Company integrations.
Policy engines.
Observability backends.
UI extensions.

The kernel should not become a giant monolith.

## 5.2 The Kernel Controls, Plugins Extend

The kernel should handle:

Identity.
Plugin lifecycle.
Permission enforcement.
Request routing.
Core state.
Audit logs.
Task orchestration.
Plugin health.
System events.

Plugins should handle:

Specific tools.
Specific models.
Specific execution runtimes.
Specific storage providers.
Specific company integrations.
Specific agent behaviors.

## 5.3 Least Privilege by Default

No agent, user, plugin, or tool should get broad access automatically.

Every meaningful action should pass through permission checks.

Default behavior should be safe.

## 5.4 One Control Plane, Many Implementations

Relux should provide one control plane, but not force one backend.

A user can choose:

SQLite for local development.
Postgres for production.
Redis for task brokering.
NATS for high-throughput messaging.
Qdrant for vector search.
ChromaDB for lightweight vector search.
Docker for execution.
WASM for sandboxed execution.
SOL runtime for SOL programs.

## 5.5 Replaceable Components

If one plugin is replaced, the entire system should not need to be rewritten.

Examples:

Replace OpenAI adapter with Anthropic adapter.
Replace SQLite provider with Postgres provider.
Replace ChromaDB with Qdrant.
Replace Docker execution with WASM execution.
Replace GitHub tool plugin with GitLab tool plugin.

## 5.6 Clear Developer Experience

The developer should be able to:

Install a plugin.
Configure credentials.
Create an agent.
Grant permissions.
Run a task.
Observe results.

The ideal feeling is:

```text
Search.
Install.
Configure.
Permission.
Use.
```

## 5.7 Clear Operator Experience

An operator should be able to:

View all agents.
View all plugins.
View plugin health.
View tasks.
View permissions.
View audit logs.
Approve risky actions.
Disable broken plugins.
Rotate secrets.
See system status.

## 5.8 Human Approval Should Be Built In

Some actions should require approval.

Examples:

Merge PR.
Delete production database row.
Send external email.
Issue refund.
Deploy to production.
Access sensitive documents.
Run shell command with high risk.

Relux should support human-in-the-loop approvals as a first-class feature.

---

# 6. Target Users

## 6.1 Individual Developer

An individual developer wants to build an AI-powered app quickly.

Pain points:

They do not want to build auth again.
They do not want to build tool calling again.
They do not want to manually connect every API.
They do not want to write separate memory code.
They do not want to create a dashboard from scratch.

Relux helps by giving them a ready control plane.

Example:

A developer wants to build a coding assistant that can read a GitHub repo, create branches, write code, run tests, and open PRs.

With Relux:

Install Anthropic adapter.
Install GitHub tools.
Install terminal tools.
Install Python execution.
Create coding agent.
Grant scoped permissions.
Run task.

## 6.2 Startup Team

A startup wants to build an AI SaaS product.

Pain points:

They need to ship fast.
They need reliable architecture.
They need model flexibility.
They need customer-level tenancy.
They need logs and permissions.
They need integrations.

Relux helps by giving them the infrastructure layer.

Example:

A startup builds an AI analyst product. Their customers connect Slack, Google Drive, Notion, and Postgres. Relux handles plugin installation, permissions, customer namespaces, task execution, and audit logs.

## 6.3 Enterprise Team

An enterprise wants agents inside internal systems.

Pain points:

They need security.
They need visibility.
They need access controls.
They need audit logs.
They need approval workflows.
They need compliance.
They need integration with existing infrastructure.

Relux helps by making agents manageable.

Example:

A company creates a support agent that can read tickets, draft responses, check policy, escalate refunds, and ask for human approval before sensitive actions.

## 6.4 DevOps or Platform Engineer

A platform engineer wants to manage agent infrastructure.

Pain points:

They need deployment control.
They need plugin health checks.
They need logs.
They need secrets management.
They need task queues.
They need rollback.
They need monitoring.

Relux helps by exposing system-level controls.

## 6.5 Plugin Developer

A plugin developer wants to build reusable capabilities.

Pain points:

They need a standard way to expose tools.
They need schemas.
They need permissions.
They need distribution.
They need installation and configuration flows.

Relux helps by giving them a plugin contract and registry.

---

# 7. Core Concepts

## 7.1 Kernel

The kernel is the minimal core of Relux.

It is responsible for:

Loading plugins.
Managing plugin lifecycle.
Storing core entities.
Authenticating users and agents.
Authorizing actions.
Routing requests to plugins.
Managing tasks.
Managing leases.
Writing audit logs.
Publishing system events.
Tracking plugin health.

The kernel should not contain business logic for specific tools or agents.

## 7.2 Plugin

A plugin is a self-contained package that extends Relux.

A plugin can provide:

An agent adapter.
A toolset.
A storage provider.
A vector database provider.
A task broker.
An execution runtime.
A memory backend.
A policy engine.
A company integration.
A UI panel.

Plugins register metadata, capabilities, schemas, health checks, permissions, and endpoints.

## 7.3 Adapter

An Adapter plugin connects Relux to a specific type of agent or model runtime.

Examples:

relix-adapter-openai
relix-adapter-anthropic
relix-adapter-ollama
relix-adapter-openrouter
relix-adapter-hermes
relix-adapter-custom-http

An adapter answers:

What model or agent runtime does this use?
What tasks can it accept?
How does it execute a task?
How does it call tools?
How does it stream results?
How does it handle failures?

## 7.4 ToolSet

A ToolSet plugin adds tools that agents can use.

Examples:

relix-tools-github
relix-tools-terminal
relix-tools-browser
relix-tools-slack
relix-tools-discord
relix-tools-tavily
relix-tools-google-drive
relix-tools-zendesk
relix-tools-salesforce

A ToolSet registers tools like:

github.create_branch
github.create_pr
github.comment_on_issue
terminal.run_command
browser.open_page
slack.send_message
drive.search_files
zendesk.read_ticket

Each tool has:

Name.
Description.
Input schema.
Output schema.
Required permissions.
Risk level.
Timeout.
Retry behavior.
Approval requirement.

## 7.5 ServiceProvider

A ServiceProvider plugin provides infrastructure.

Examples:

relix-provider-sqlite
relix-provider-postgres
relix-provider-redis
relix-provider-nats
relix-provider-qdrant
relix-provider-chromadb
relix-provider-s3
relix-provider-localfs

ServiceProviders can implement:

PrimaryStorage.
VectorStore.
TaskBroker.
BlobStorage.
MemoryStore.
SecretStore.
EventBus.

## 7.6 ExecutionEnvironment

An ExecutionEnvironment plugin runs code or programs.

Examples:

relix-env-python-wasm
relix-env-node-wasm
relix-env-docker
relix-env-firecracker
relix-env-sol
relix-env-shell
relix-env-browser

Execution environments must be sandboxed where possible.

They should declare:

Supported languages.
Resource limits.
Network access policy.
Filesystem access policy.
Timeouts.
Allowed packages.
Risk level.
Isolation mode.

## 7.7 Agent

An Agent is a configured actor inside Relux.

An agent has:

Name.
Description.
Adapter.
Model configuration.
Role.
Persona.
Permissions.
Tools.
Memory configuration.
Execution permissions.
Namespace.
Owner.
Status.
Audit history.

Example:

```yaml
agents:
  - name: "code-agent"
    description: "Writes code, runs tests, and opens pull requests."
    adapter:
      plugin: relix-adapter-anthropic
      config:
        model: "claude-sonnet"
    permissions:
      - "tool:relix-tools-github:create_branch"
      - "tool:relix-tools-github:create_pr"
      - "tool:relix-tools-terminal:run_readonly"
      - "exec:relix-env-python-wasm:run"
```

## 7.8 Task

A Task is a unit of work assigned to an agent or workflow.

A task includes:

Task ID.
Title.
Input.
Requested by.
Assigned agent.
Status.
Priority.
Required permissions.
Context.
Deadline.
Logs.
Tool calls.
Outputs.
Approvals.

Statuses:

created
queued
leased
running
waiting_for_tool
waiting_for_approval
completed
failed
cancelled
expired

## 7.9 Lease

A Lease prevents multiple agents from accidentally working on the same task at the same time.

A lease includes:

Task ID.
Agent ID.
Lease start.
Lease expiration.
Heartbeat.
Renewal policy.
Failure behavior.

If an agent stops heartbeating, the lease expires and the task can be reassigned.

## 7.10 Namespace

A Namespace isolates resources.

Examples:

Company namespace.
Team namespace.
Project namespace.
Environment namespace.
Customer namespace.

A namespace contains:

Agents.
Users.
Plugins.
Permissions.
Tasks.
Secrets.
Logs.
Configurations.

Example:

```text
acme-corp
  /engineering
  /support
  /legal
  /production
  /staging
```

## 7.11 Permission

A Permission is a string that describes what an actor can do.

Examples:

```text
tool:relix-tools-github:create_pr
tool:relix-tools-github:merge_pr
tool:relix-tools-terminal:run_command
exec:relix-env-python-wasm:run
provider:relix-provider-postgres:read
provider:relix-provider-postgres:write
agent:code-agent:assign_task
plugin:relix-tools-github:configure
```

Permissions can be attached to:

Users.
Roles.
Agents.
Teams.
Namespaces.
Plugins.

## 7.12 Audit Log

The audit log records important actions.

Examples:

User installed plugin.
Agent called tool.
Permission was granted.
Permission was denied.
Task was created.
Task was completed.
Plugin failed health check.
Secret was rotated.
Human approved an action.
Human rejected an action.

Audit logs should be immutable.

---

# 8. High-Level Architecture

## 8.1 Architecture Overview

Relux is composed of:

1. Relux Kernel
2. Plugin Host
3. Plugin Registry
4. Core State Store
5. Permission Engine
6. Task Orchestrator
7. Request Router
8. Event Bus
9. Dashboard
10. CLI
11. SDKs
12. Plugin Runtime

Conceptual structure:

```text
+------------------------------------------------------+
|                    Relux Dashboard                   |
| Agents | Plugins | Tasks | Logs | Permissions | Runs |
+---------------------------+--------------------------+
                            |
                            v
+------------------------------------------------------+
|                    Relux REST API                    |
|       Management, configuration, users, tasks         |
+---------------------------+--------------------------+
                            |
                            v
+------------------------------------------------------+
|                     Relux Kernel                     |
|------------------------------------------------------|
| Plugin Lifecycle | AuthN/AuthZ | Routing | State     |
| Task Orchestrator | Audit Log | Events | Health      |
+---------------------------+--------------------------+
                            |
                            v
+------------------------------------------------------+
|                    Plugin Host                       |
|------------------------------------------------------|
| Adapter Plugins | ToolSet Plugins | Provider Plugins |
| Execution Environment Plugins | Policy Plugins       |
+------------------------------------------------------+
                            |
                            v
+------------------------------------------------------+
|                 External Systems                     |
| OpenAI | Claude | GitHub | Postgres | Qdrant | Slack |
| Docker | Browser | SOL Runtime | Internal APIs       |
+------------------------------------------------------+
```

## 8.2 Kernel Responsibilities

The kernel must do only the things that every Relux system needs.

### Kernel owns:

Plugin installation records.
Plugin lifecycle state.
Core entities.
Users.
Agents.
Namespaces.
Tasks.
Leases.
Roles.
Permissions.
Audit logs.
Request routing.
Authentication.
Authorization.
Plugin health.
System events.

### Kernel does not own:

Specific model logic.
Specific GitHub logic.
Specific database implementation.
Specific vector store logic.
Specific browser automation logic.
Specific Python runtime implementation.
Specific company integration logic.

Those belong to plugins.

## 8.3 Plugin Host Responsibilities

The Plugin Host is responsible for running plugins safely.

It should:

Load plugins.
Validate plugin manifests.
Start plugin processes or WASM modules.
Manage plugin lifecycle.
Expose plugin capabilities to kernel.
Run health checks.
Apply sandbox policies.
Restart failed plugins.
Report plugin status.

The plugin host may support multiple runtime modes:

In-process plugins.
Out-of-process plugins.
WASM plugins.
Containerized plugins.
Remote plugins over HTTP or gRPC.

## 8.4 Plugin Registry Responsibilities

The Plugin Registry stores available plugins.

It should support:

Search.
Install.
Versioning.
Verification.
Signatures.
Metadata.
Compatibility checks.
Dependency resolution.
Private registries.
Official plugins.
Community plugins.
Company-internal plugins.

Example CLI flow:

```bash
relix plugins search github
relix plugins install relix-tools-github
relix plugins configure relix-tools-github
relix plugins enable relix-tools-github
relix plugins health relix-tools-github
```

## 8.5 Request Router Responsibilities

The Request Router receives calls and routes them to the correct destination.

Examples:

Agent requests a tool call.
Kernel checks permission.
Router sends request to ToolSet plugin.
Plugin returns result.
Kernel logs action.
Agent receives tool result.

The router should support:

REST.
gRPC.
WebSockets.
Internal message bus.
Task broker.

---

# 9. Plugin System

## 9.1 Plugin Design Goal

The plugin system should make Relux extensible without changing the kernel.

A plugin should be easy to:

Build.
Package.
Install.
Configure.
Test.
Permission.
Disable.
Upgrade.
Remove.
Publish.

## 9.2 Plugin Manifest

Every plugin should include a manifest.

Example:

```yaml
name: relix-tools-github
version: 0.1.0
type: ToolSet
description: Adds GitHub tools for agents.
author: Relux Labs
license: Apache-2.0

runtime:
  mode: wasm
  entrypoint: plugin.wasm

capabilities:
  tools:
    - name: github.create_branch
      description: Creates a new branch in a repository.
      risk: low
      permission: tool:relix-tools-github:create_branch
    - name: github.create_pr
      description: Opens a pull request.
      risk: medium
      permission: tool:relix-tools-github:create_pr
    - name: github.merge_pr
      description: Merges a pull request.
      risk: high
      permission: tool:relix-tools-github:merge_pr
      requires_approval: true

config_schema:
  type: object
  required:
    - token
  properties:
    token:
      type: string
      secret: true
    default_owner:
      type: string

health:
  endpoint: /health
  interval_seconds: 30

permissions:
  - tool:relix-tools-github:create_branch
  - tool:relix-tools-github:create_pr
  - tool:relix-tools-github:merge_pr

dependencies: []
```

## 9.3 Plugin Types

Relux should support these first-class plugin types:

1. Adapter
2. ToolSet
3. ServiceProvider
4. ExecutionEnvironment
5. MemoryProvider
6. PolicyProvider
7. ObservabilityProvider
8. UIExtension
9. WorkflowExtension
10. IntegrationBridge

The original four are the foundation, but the extended types make the ecosystem clearer.

---

# 10. Plugin Extension Points

## 10.1 Adapter Plugin

### Purpose

An Adapter plugin lets Relux use a model provider, agent runtime, or external agent framework.

### Examples

```text
relix-adapter-openai
relix-adapter-anthropic
relix-adapter-gemini
relix-adapter-grok
relix-adapter-openrouter
relix-adapter-ollama
relix-adapter-hermes
relix-adapter-custom-http
```

### Adapter Responsibilities

An adapter should:

Accept task input from the kernel.
Create model or agent requests.
Stream output back to the kernel.
Request tool calls through the kernel.
Handle model-specific formats.
Report errors.
Support cancellation.
Support retries if safe.

### Example Adapter Flow

User creates a task:

```text
"Review this repo and open a PR fixing the failing test."
```

Kernel assigns it to code-agent.

code-agent uses:

```text
relix-adapter-anthropic
```

The adapter sends the task to Claude.

Claude requests a tool call:

```text
github.read_repo
```

Kernel checks permissions.

If allowed, kernel routes to GitHub ToolSet.

GitHub returns files.

Claude edits code and requests:

```text
github.create_pr
```

Kernel checks permission.

If allowed, PR is created.

If the agent requests:

```text
github.merge_pr
```

Kernel checks permission and may require human approval.

## 10.2 ToolSet Plugin

### Purpose

A ToolSet plugin gives agents callable tools.

### ToolSet Examples

```text
relix-tools-github
relix-tools-terminal
relix-tools-browser
relix-tools-slack
relix-tools-discord
relix-tools-google-drive
relix-tools-notion
relix-tools-zendesk
relix-tools-salesforce
relix-tools-linear
relix-tools-jira
relix-tools-email
relix-tools-calendar
relix-tools-tavily
```

### Tool Definition Example

```json
{
  "name": "github.create_pr",
  "description": "Create a pull request in a GitHub repository.",
  "input_schema": {
    "type": "object",
    "required": ["owner", "repo", "branch", "base", "title", "body"],
    "properties": {
      "owner": { "type": "string" },
      "repo": { "type": "string" },
      "branch": { "type": "string" },
      "base": { "type": "string" },
      "title": { "type": "string" },
      "body": { "type": "string" }
    }
  },
  "output_schema": {
    "type": "object",
    "properties": {
      "pr_url": { "type": "string" },
      "number": { "type": "integer" }
    }
  },
  "permission": "tool:relix-tools-github:create_pr",
  "risk": "medium",
  "requires_approval": false
}
```

## 10.3 ServiceProvider Plugin

### Purpose

A ServiceProvider plugin supplies backend infrastructure.

### Interfaces

PrimaryStorage.
VectorStore.
TaskBroker.
BlobStorage.
SecretStore.
EventBus.
CacheStore.

### Example

A developer starts locally using SQLite:

```yaml
providers:
  primary_storage:
    plugin: relix-provider-sqlite
    config:
      path: "./relix.db"
```

Later, they move to production with Postgres:

```yaml
providers:
  primary_storage:
    plugin: relix-provider-postgres
    config:
      connection_string: "${POSTGRES_URL}"
```

The application should not need to rewrite business logic.

## 10.4 ExecutionEnvironment Plugin

### Purpose

An ExecutionEnvironment plugin runs code or programs in controlled environments.

### Examples

```text
relix-env-python-wasm
relix-env-node-wasm
relix-env-docker
relix-env-sol
relix-env-shell
relix-env-browser
```

### Example Permission

```text
exec:relix-env-python-wasm:run
```

### Example Runtime Config

```yaml
execution_environments:
  - plugin: relix-env-python-wasm
    config:
      max_memory_mb: 256
      timeout_seconds: 30
      network: false
      filesystem: "ephemeral"
```

### Example Use Case

A data analysis agent needs to run Python.

Agent asks:

```text
Run this Python code to calculate the result.
```

Kernel checks:

```text
Does this agent have exec:relix-env-python-wasm:run?
```

If yes, route to Python WASM environment.

If no, deny and log.

## 10.5 MemoryProvider Plugin

### Purpose

MemoryProvider plugins store and retrieve agent memory.

Examples:

```text
relix-memory-local
relix-memory-postgres
relix-memory-qdrant
relix-memory-chromadb
relix-memory-pinecone
```

Memory can include:

Short-term task context.
Long-term agent preferences.
Company knowledge snippets.
Project history.
Conversation summaries.
Embeddings.
Retrieved documents.

Memory should be namespace-aware and permission-aware.

An agent should not retrieve memory from a namespace it cannot access.

## 10.6 PolicyProvider Plugin

### Purpose

PolicyProvider plugins enforce advanced rules.

Basic permissions may be enough for simple systems, but companies may need policy logic.

Examples:

```text
relix-policy-opa
relix-policy-cedar
relix-policy-custom
```

Policies can answer:

Can this agent access this file?
Can this agent send this message externally?
Can this task run in production?
Does this action require approval?
Is this user allowed to install plugins?
Can this plugin access network?

## 10.7 ObservabilityProvider Plugin

### Purpose

ObservabilityProvider plugins export logs, metrics, and traces.

Examples:

```text
relix-observe-opentelemetry
relix-observe-datadog
relix-observe-prometheus
relix-observe-local
```

They should track:

Task duration.
Tool call latency.
Plugin error rate.
Agent success rate.
Permission denials.
Approval wait time.
Token usage.
Model cost.
Execution failures.

## 10.8 UIExtension Plugin

### Purpose

UIExtension plugins add dashboard panels.

Example:

The GitHub ToolSet plugin could add a GitHub settings page.

The browser plugin could add a browser session viewer.

The task broker plugin could add queue metrics.

The SOL runtime plugin could add a SOL execution trace viewer.

UI extensions must be sandboxed and permissioned.

---

# 11. Core Data Model

## 11.1 Company

Represents a tenant or organization.

Fields:

```text
id
name
slug
created_at
updated_at
status
default_namespace_id
billing_plan
settings
```

## 11.2 Namespace

Represents an isolated scope.

Fields:

```text
id
company_id
parent_namespace_id
name
slug
type
created_at
updated_at
settings
```

Types:

company
team
project
environment
customer
personal

## 11.3 User

Represents a human user.

Fields:

```text
id
company_id
email
name
status
auth_provider
created_at
updated_at
last_login_at
```

## 11.4 Agent

Represents an AI actor.

Fields:

```text
id
company_id
namespace_id
name
description
adapter_plugin_id
adapter_config_ref
persona
status
created_by
created_at
updated_at
```

Status:

draft
active
paused
disabled
error

## 11.5 PluginRegistration

Represents an installed plugin.

Fields:

```text
id
company_id
namespace_id
name
version
type
source
status
manifest
config_ref
health_status
installed_by
installed_at
updated_at
```

Status:

installed
configured
enabled
disabled
failed
upgrading
removed

## 11.6 Task

Represents work to be done.

Fields:

```text
id
company_id
namespace_id
title
description
input
status
priority
created_by_type
created_by_id
assigned_agent_id
created_at
started_at
completed_at
deadline_at
output
error
metadata
```

## 11.7 Lease

Represents task ownership.

Fields:

```text
id
task_id
agent_id
lease_token
expires_at
last_heartbeat_at
status
created_at
```

## 11.8 ToolCall

Represents a tool call made by an agent.

Fields:

```text
id
task_id
agent_id
plugin_id
tool_name
input
output
status
risk_level
permission
approval_id
started_at
completed_at
error
```

## 11.9 Role

Represents a group of permissions.

Fields:

```text
id
company_id
namespace_id
name
description
permissions
created_at
updated_at
```

## 11.10 PermissionGrant

Represents a permission assigned to a user, role, agent, or plugin.

Fields:

```text
id
company_id
namespace_id
subject_type
subject_id
permission
conditions
created_by
created_at
expires_at
```

Subject types:

user
role
agent
plugin
team

## 11.11 ApprovalRequest

Represents a human approval gate.

Fields:

```text
id
company_id
namespace_id
task_id
tool_call_id
requested_by_agent_id
action
risk_level
input_summary
status
approved_by
approved_at
rejected_by
rejected_at
reason
expires_at
```

Status:

pending
approved
rejected
expired
cancelled

## 11.12 AuditLog

Represents immutable system history.

Fields:

```text
id
company_id
namespace_id
actor_type
actor_id
action
target_type
target_id
result
timestamp
metadata
ip_address
request_id
```

---

# 12. Permission Model

## 12.1 Permission Philosophy

Relux permissions should be explicit, human-readable, and composable.

A permission string should answer:

```text
What category is being accessed?
Which plugin or resource is involved?
Which action is allowed?
```

Format:

```text
category:resource:action
```

Examples:

```text
tool:relix-tools-github:create_pr
tool:relix-tools-github:merge_pr
exec:relix-env-python-wasm:run
plugin:relix-tools-github:configure
agent:code-agent:assign_task
task:namespace-engineering:create
memory:project-alpha:read
provider:relix-provider-postgres:read
```

## 12.2 Permission Scope

Permissions can be scoped by:

Company.
Namespace.
Project.
Environment.
Agent.
User.
Role.
Plugin.
Resource.

Example:

```yaml
permissions:
  - permission: "tool:relix-tools-github:create_pr"
    scope:
      repo: "acme/api"
      branches:
        - "feature/*"
        - "fix/*"
```

## 12.3 Wildcards

Wildcards should be supported carefully.

Example:

```text
tool:relix-tools-github:*
```

This grants all GitHub tool actions.

Wildcards should be discouraged for production agents unless explicitly approved.

## 12.4 Risk Levels

Every tool/action should declare risk.

Risk levels:

low
medium
high
critical

Examples:

Low:

Read repository files.
Search web.
Read documentation.
List issues.

Medium:

Create branch.
Open pull request.
Send internal Slack message.
Run non-networked Python code.

High:

Merge pull request.
Send external email.
Deploy to staging.
Modify database rows.

Critical:

Deploy to production.
Delete database table.
Rotate secrets.
Issue refund.
Access sensitive legal or financial records.

## 12.5 Approval Rules

Actions can require approval based on:

Risk level.
Agent identity.
Namespace.
Environment.
Resource.
Time.
User policy.

Example:

```yaml
approval_rules:
  - match:
      permission: "tool:relix-tools-github:merge_pr"
      environment: "production"
    require:
      approvals: 1
      role: "engineering-manager"

  - match:
      permission: "exec:relix-env-shell:run"
      risk: "critical"
    require:
      approvals: 2
      roles:
        - "security-admin"
        - "platform-admin"
```

## 12.6 Permission Example: Coding Agent

Allowed:

```text
github.read_repo
github.create_branch
github.create_pr
terminal.run_tests
python.run
```

Not allowed:

```text
github.merge_pr
github.delete_repo
terminal.rm_rf
database.drop_table
secrets.read_all
```

## 12.7 Permission Example: Support Agent

Allowed:

```text
zendesk.read_ticket
zendesk.draft_reply
policy.search
slack.send_internal_message
```

Requires approval:

```text
zendesk.send_reply
stripe.issue_refund
email.send_external
```

Not allowed:

```text
database.export_customers
secrets.read
production.deploy
```

---

# 13. Task Orchestration

## 13.1 Task Lifecycle

A task moves through stages:

```text
created
queued
leased
running
waiting_for_tool
waiting_for_approval
completed
failed
cancelled
expired
```

## 13.2 Task Creation

A task can be created by:

User.
API.
Agent.
Workflow.
Webhook.
Scheduled job.
External system.

Example API request:

```json
{
  "title": "Fix failing unit test",
  "description": "Investigate the failing test in the billing module and open a PR.",
  "agent": "code-agent",
  "priority": "high",
  "input": {
    "repo": "acme/billing",
    "test": "billing_invoice_test"
  }
}
```

## 13.3 Task Assignment

The kernel can assign tasks using:

Explicit assignment.
Capability matching.
Load balancing.
Priority queue.
Manager agent delegation.
Policy-based routing.

Example:

```text
Task: "Find legal risk in this contract"
Required capability: legal_review
Assigned to: legal-agent
```

## 13.4 Manager Agent Pattern

Relux should support a manager agent that delegates work.

Example:

User asks:

```text
Prepare a release for version 2.1.0.
```

Manager agent breaks it into subtasks:

Research agent:

```text
Review changelog and release notes.
```

Code agent:

```text
Run tests and check failing builds.
```

Docs agent:

```text
Update documentation.
```

GitHub agent:

```text
Open release PR.
```

Deployment agent:

```text
Prepare staging deployment.
```

The manager agent does not need permission to do every action itself. It needs permission to assign subtasks to agents that have the correct permissions.

## 13.5 Lease and Heartbeat

When an agent starts a task, it receives a lease.

If the agent continues working, it heartbeats.

If the agent crashes, the lease expires.

The kernel can then:

Retry with same agent.
Reassign to another agent.
Mark task failed.
Ask human operator.

## 13.6 Tool Call Flow

Detailed flow:

```text
1. Agent receives task.
2. Agent decides it needs a tool.
3. Agent sends tool call request to kernel.
4. Kernel identifies plugin and tool.
5. Kernel checks agent permission.
6. Kernel checks policy rules.
7. Kernel checks whether approval is required.
8. If approval required, task waits.
9. If approved, kernel routes call to plugin.
10. Plugin executes action.
11. Plugin returns result.
12. Kernel logs tool call.
13. Kernel sends result back to agent.
14. Agent continues task.
```

## 13.7 Example Tool Call

Agent wants to create a PR:

```json
{
  "tool": "github.create_pr",
  "input": {
    "owner": "acme",
    "repo": "billing",
    "branch": "fix-invoice-test",
    "base": "main",
    "title": "Fix invoice test failure",
    "body": "This PR fixes the failing invoice unit test."
  }
}
```

Kernel checks:

```text
Does agent have tool:relix-tools-github:create_pr?
Is repo within allowed scope?
Does action require approval?
Is GitHub plugin healthy?
```

Then routes call.

## 13.8 Failure Handling

Failures should be structured.

Failure types:

permission_denied
plugin_unavailable
plugin_timeout
tool_error
model_error
execution_error
approval_rejected
lease_expired
rate_limited
invalid_input
policy_denied

Example response:

```json
{
  "status": "failed",
  "error_type": "permission_denied",
  "message": "Agent code-agent does not have permission tool:relix-tools-github:merge_pr.",
  "recoverable": true,
  "suggested_action": "Request approval or assign task to an agent with merge permission."
}
```

---

# 14. Session and Process Model

## 14.1 Why Sessions Matter

Agentic systems need state.

A task may involve:

Multiple steps.
Multiple agents.
Multiple tool calls.
Memory.
Temporary files.
Execution results.
Context.
Logs.
Approvals.

A session provides a stateful execution context.

## 14.2 Session Definition

A Session is a stateful execution container for a task, workflow, or program.

A session contains:

Task context.
Agent context.
Memory references.
Execution state.
Tool call history.
Temporary files.
Process list.
Permissions snapshot.
Audit trail.

## 14.3 Process Definition

A Process is a unit of execution inside a session.

A process can be:

Agent reasoning loop.
Tool call.
SOL program execution.
Python execution.
Browser task.
Subtask.
Workflow branch.

## 14.4 Nested Process Model

Instead of creating a completely separate program for every unit of work, Relux should allow a session to spin up multiple processes.

Example:

```text
Session: release-prep-2.1.0
  Process 1: research changelog
  Process 2: run tests
  Process 3: update docs
  Process 4: create PR
  Process 5: prepare deployment
```

This allows scaling without creating separate isolated programs for every small action.

## 14.5 SOL Execution Session

A SOL program can run inside a session.

Example:

```text
Session: customer-support-flow
  Process: run support_workflow.sol
    Process: classify ticket
    Process: search knowledge base
    Process: draft reply
    Process: request approval
```

## 14.6 Session Benefits

Sessions give Relux:

Better scaling.
Better state tracking.
Better observability.
Better failure recovery.
Better parallel execution.
Better workflow control.
Better debugging.

---

# 15. Configuration System

## 15.1 relix.yaml Philosophy

The entire system should be configurable through a single file for developers, while still manageable through the dashboard for operators.

The file should define:

Company.
Namespaces.
Providers.
Plugins.
Agents.
Permissions.
Approval rules.
Execution environments.
Memory.
Observability.

## 15.2 Full relix.yaml Example

```yaml
company_id: "acme-corp"

namespaces:
  - name: "engineering"
    type: "team"

  - name: "support"
    type: "team"

  - name: "production"
    type: "environment"

providers:
  primary_storage:
    plugin: relix-provider-postgres
    config:
      connection_string: "${POSTGRES_URL}"

  task_broker:
    plugin: relix-provider-redis
    config:
      url: "${REDIS_URL}"

  vector_store:
    plugin: relix-provider-qdrant
    config:
      url: "http://localhost:6333"

plugins:
  - name: relix-adapter-anthropic
    version: "0.1.0"
    config:
      api_key: "${ANTHROPIC_API_KEY}"

  - name: relix-tools-github
    version: "0.1.0"
    config:
      token: "${GITHUB_TOKEN}"
      default_owner: "acme"

  - name: relix-tools-terminal
    version: "0.1.0"
    config:
      allowed_workspaces:
        - "/workspace/acme"

  - name: relix-env-python-wasm
    version: "0.1.0"
    config:
      max_memory_mb: 256
      timeout_seconds: 30
      network: false

agents:
  - name: "manager-agent"
    namespace: "engineering"
    description: "Breaks large engineering tasks into subtasks and delegates them."
    adapter:
      plugin: relix-adapter-anthropic
      config:
        model: "claude-sonnet"
    persona: |
      You are a careful engineering manager agent.
      You break tasks into subtasks.
      You do not perform risky actions directly.
      You delegate work to specialized agents.
    permissions:
      - "task:engineering:create"
      - "task:engineering:assign"
      - "agent:code-agent:assign_task"
      - "agent:docs-agent:assign_task"

  - name: "code-agent"
    namespace: "engineering"
    description: "Writes code, runs tests, and opens pull requests."
    adapter:
      plugin: relix-adapter-anthropic
      config:
        model: "claude-sonnet"
    permissions:
      - "tool:relix-tools-github:read_repo"
      - "tool:relix-tools-github:create_branch"
      - "tool:relix-tools-github:create_pr"
      - "tool:relix-tools-terminal:run_tests"
      - "exec:relix-env-python-wasm:run"

  - name: "docs-agent"
    namespace: "engineering"
    description: "Updates documentation and changelogs."
    adapter:
      plugin: relix-adapter-anthropic
      config:
        model: "claude-haiku"
    permissions:
      - "tool:relix-tools-github:read_repo"
      - "tool:relix-tools-github:create_branch"
      - "tool:relix-tools-github:create_pr"

approval_rules:
  - name: "Require approval for merging PRs"
    match:
      permission: "tool:relix-tools-github:merge_pr"
    require:
      approvals: 1
      role: "engineering-manager"

  - name: "Require approval for production deploy"
    match:
      permission: "tool:relix-tools-deploy:production"
    require:
      approvals: 2
      roles:
        - "platform-admin"
        - "engineering-manager"

observability:
  provider:
    plugin: relix-observe-opentelemetry
    config:
      endpoint: "${OTEL_ENDPOINT}"
```

---

# 16. CLI Specification

## 16.1 CLI Goals

The CLI should let developers manage Relux from the terminal.

It should support:

Initialization.
Plugin search.
Plugin installation.
Configuration.
Agent creation.
Task creation.
Task status.
Logs.
Permissions.
Local development.

## 16.2 Commands

### Initialize Project

```bash
relix init
```

Creates:

```text
relix.yaml
.relix/
plugins/
```

### Start Relux

```bash
relix up
```

Starts local kernel, plugin host, and dashboard.

### Stop Relux

```bash
relix down
```

### Plugin Search

```bash
relix plugins search github
```

### Plugin Install

```bash
relix plugins install relix-tools-github
```

### Plugin Configure

```bash
relix plugins configure relix-tools-github
```

### Plugin List

```bash
relix plugins list
```

Example output:

```text
NAME                      TYPE        VERSION   STATUS    HEALTH
relix-tools-github        ToolSet     0.1.0     enabled   healthy
relix-adapter-anthropic   Adapter     0.1.0     enabled   healthy
relix-provider-sqlite     Provider    0.1.0     enabled   healthy
```

### Create Agent

```bash
relix agents create code-agent \
  --adapter relix-adapter-anthropic \
  --model claude-sonnet
```

### Grant Permission

```bash
relix permissions grant code-agent tool:relix-tools-github:create_pr
```

### Run Task

```bash
relix tasks create \
  --agent code-agent \
  --title "Fix failing tests" \
  --input "Run tests and open a PR with the fix."
```

### View Task

```bash
relix tasks get task_123
```

### Tail Logs

```bash
relix logs tail
```

### View Audit

```bash
relix audit list --agent code-agent
```

---

# 17. REST API Specification

## 17.1 API Philosophy

The REST API should support management and compatibility.

Used by:

Dashboard.
CLI.
External apps.
Operators.
Developers.

## 17.2 Core Endpoints

### Health

```http
GET /health
```

### Plugins

```http
GET /plugins
GET /plugins/{plugin_id}
POST /plugins/install
POST /plugins/{plugin_id}/configure
POST /plugins/{plugin_id}/enable
POST /plugins/{plugin_id}/disable
DELETE /plugins/{plugin_id}
GET /plugins/{plugin_id}/health
```

### Agents

```http
GET /agents
POST /agents
GET /agents/{agent_id}
PATCH /agents/{agent_id}
DELETE /agents/{agent_id}
POST /agents/{agent_id}/permissions
GET /agents/{agent_id}/tasks
```

### Tasks

```http
GET /tasks
POST /tasks
GET /tasks/{task_id}
POST /tasks/{task_id}/cancel
GET /tasks/{task_id}/logs
GET /tasks/{task_id}/tool-calls
```

### Permissions

```http
GET /permissions
POST /permissions/grant
POST /permissions/revoke
GET /roles
POST /roles
PATCH /roles/{role_id}
```

### Approvals

```http
GET /approvals
GET /approvals/{approval_id}
POST /approvals/{approval_id}/approve
POST /approvals/{approval_id}/reject
```

### Audit Logs

```http
GET /audit
GET /audit/{audit_id}
```

### Namespaces

```http
GET /namespaces
POST /namespaces
GET /namespaces/{namespace_id}
PATCH /namespaces/{namespace_id}
DELETE /namespaces/{namespace_id}
```

## 17.3 Create Task Example

Request:

```json
{
  "title": "Create PR for failing tests",
  "agent": "code-agent",
  "input": {
    "repo": "acme/billing",
    "branch_prefix": "fix",
    "instructions": "Run tests, identify the failing invoice test, fix it, and open a PR."
  },
  "priority": "high"
}
```

Response:

```json
{
  "task_id": "task_01JABC",
  "status": "queued",
  "assigned_agent": "code-agent"
}
```

---

# 18. gRPC Protocol

## 18.1 Purpose

gRPC should be used for high-throughput internal communication.

Used by:

Agents.
Plugins.
Task streaming.
Tool calls.
Execution environments.
Event streams.

## 18.2 Services

```proto
service AgentRuntime {
  rpc RunTask(RunTaskRequest) returns (stream RunTaskEvent);
  rpc CancelTask(CancelTaskRequest) returns (CancelTaskResponse);
}

service ToolRuntime {
  rpc ExecuteTool(ExecuteToolRequest) returns (ExecuteToolResponse);
}

service PluginRuntime {
  rpc GetManifest(GetManifestRequest) returns (PluginManifest);
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
}

service TaskBroker {
  rpc LeaseTask(LeaseTaskRequest) returns (LeaseTaskResponse);
  rpc Heartbeat(HeartbeatRequest) returns (HeartbeatResponse);
  rpc CompleteTask(CompleteTaskRequest) returns (CompleteTaskResponse);
}
```

---

# 19. WebSocket Events

## 19.1 Purpose

WebSockets should power real-time dashboard updates.

Events:

task.created
task.started
task.completed
task.failed
tool_call.started
tool_call.completed
tool_call.failed
approval.created
approval.approved
approval.rejected
plugin.installed
plugin.enabled
plugin.failed
agent.started
agent.paused
audit.created

## 19.2 Example Event

```json
{
  "type": "tool_call.completed",
  "timestamp": "2026-06-08T12:00:00Z",
  "data": {
    "task_id": "task_123",
    "agent": "code-agent",
    "tool": "github.create_pr",
    "status": "completed",
    "output": {
      "pr_url": "https://github.com/acme/billing/pull/42"
    }
  }
}
```

---

# 20. Dashboard Specification

## 20.1 Dashboard Goal

The dashboard should make Relux feel like an operating center for agents and plugins.

It should not feel like a basic chatbot UI.

It should feel like:

Control room.
Agent manager.
Plugin marketplace.
Permission console.
Task monitor.
Audit center.
Developer workspace.

## 20.2 Main Navigation

Suggested sidebar:

```text
Overview
Agents
Tasks
Plugins
Tools
Workflows
Approvals
Memory
Namespaces
Permissions
Audit Logs
Settings
```

## 20.3 Overview Page

Shows:

Total agents.
Running tasks.
Pending approvals.
Plugin health.
Recent failures.
Tool call volume.
Token usage.
Estimated cost.
System status.

Example cards:

```text
Active Agents: 8
Running Tasks: 13
Pending Approvals: 4
Healthy Plugins: 21/23
Failed Tool Calls: 3
```

## 20.4 Agents Page

Shows all agents.

Columns:

Name.
Namespace.
Adapter.
Status.
Current task.
Permissions count.
Last active.
Success rate.

Clicking an agent opens:

Profile.
Persona.
Adapter config.
Tools.
Permissions.
Memory.
Task history.
Logs.
Audit history.

## 20.5 Agent Detail Page

Sections:

Identity.
Instructions/persona.
Model/adapter.
Granted tools.
Denied tools.
Approval rules.
Memory access.
Execution permissions.
Task queue.
Recent tool calls.
Risk profile.

Example:

```text
Agent: code-agent
Adapter: relix-adapter-anthropic
Model: claude-sonnet
Namespace: engineering
Status: active

Allowed:
- github.read_repo
- github.create_branch
- github.create_pr
- terminal.run_tests
- python.run

Requires approval:
- github.merge_pr

Denied:
- github.delete_repo
- database.drop_table
- secrets.read_all
```

## 20.6 Plugins Page

Shows installed plugins.

Columns:

Name.
Type.
Version.
Status.
Health.
Last check.
Permissions exposed.
Update available.

Actions:

Install.
Enable.
Disable.
Configure.
Upgrade.
View manifest.
View logs.
Uninstall.

## 20.7 Plugin Marketplace

Searchable registry.

Filters:

Adapter.
ToolSet.
ServiceProvider.
ExecutionEnvironment.
MemoryProvider.
Official.
Community.
Private.
Verified.

Plugin card:

```text
relix-tools-github
Adds GitHub tools for agents.
Type: ToolSet
Author: Relux Labs
Verified: Yes
Tools: 12
Risk: Medium
Install
```

## 20.8 Tasks Page

Shows task queue.

Columns:

Task.
Agent.
Status.
Priority.
Created by.
Started.
Duration.
Waiting on.

Filters:

Running.
Waiting approval.
Failed.
Completed.
By agent.
By namespace.

## 20.9 Task Detail Page

Shows:

Task input.
Assigned agent.
Status timeline.
Reasoning summary.
Tool calls.
Approvals.
Logs.
Output.
Errors.
Artifacts.

Timeline example:

```text
12:00 Task created
12:01 Leased by code-agent
12:02 Tool call: github.read_repo
12:05 Tool call: terminal.run_tests
12:09 Tool call: github.create_branch
12:11 Tool call: github.create_pr
12:12 Task completed
```

## 20.10 Approvals Page

Shows pending approvals.

Approval card:

```text
Agent: code-agent
Requested action: github.merge_pr
Risk: High
Repo: acme/billing
PR: #42
Reason: Agent says tests passed and PR is ready to merge.

Approve
Reject
Ask for changes
```

## 20.11 Permissions Page

Allows managing:

Roles.
Agent permissions.
User permissions.
Plugin permissions.
Namespace permissions.
Approval rules.

Should support:

Visual permission builder.
Search permissions.
Risk warnings.
Diff view before saving.
Templates.

## 20.12 Audit Logs Page

Searchable immutable log.

Filters:

Actor.
Agent.
User.
Plugin.
Action.
Result.
Date.
Namespace.
Risk.

Example:

```text
2026-06-08 12:12
Agent code-agent called github.create_pr
Result: success
Task: Fix failing tests
PR: https://github.com/acme/billing/pull/42
```

---

# 21. Developer Experience

## 21.1 Ideal First-Time Developer Experience

A developer should be able to go from zero to running an agent quickly.

Example:

```bash
relix init
relix plugins install relix-adapter-anthropic
relix plugins install relix-tools-github
relix plugins install relix-provider-sqlite
relix agents create code-agent --adapter relix-adapter-anthropic
relix permissions grant code-agent tool:relix-tools-github:create_pr
relix up
relix tasks create --agent code-agent --title "Open a PR updating README"
```

## 21.2 SDKs

Relux should provide SDKs for:

TypeScript.
Python.
Rust.

SDK goals:

Create tasks.
Register plugins.
Call Relux API.
Build ToolSet plugins.
Build Adapter plugins.
Build ServiceProvider plugins.
Stream events.

## 21.3 TypeScript SDK Example

```ts
import { ReluxPlugin, tool } from "@relix/sdk";

export default new ReluxPlugin({
  name: "relix-tools-weather",
  type: "ToolSet",
  tools: [
    tool({
      name: "weather.get_forecast",
      description: "Get weather forecast for a city.",
      permission: "tool:relix-tools-weather:get_forecast",
      risk: "low",
      input: {
        city: "string"
      },
      async run(input) {
        return {
          city: input.city,
          forecast: "Sunny"
        };
      }
    })
  ]
});
```

## 21.4 Python SDK Example

```python
from relix import ToolSetPlugin, tool

plugin = ToolSetPlugin(
    name="relix-tools-weather",
    version="0.1.0"
)

@tool(
    name="weather.get_forecast",
    permission="tool:relix-tools-weather:get_forecast",
    risk="low"
)
def get_forecast(city: str):
    return {
        "city": city,
        "forecast": "Sunny"
    }
```

---

# 22. Plugin Security

## 22.1 Plugin Trust Levels

Plugins should have trust levels.

Trust levels:

official
verified
community
private
local
unknown

## 22.2 Plugin Sandboxing

Plugins should run with limited access.

Sandbox controls:

Network access.
Filesystem access.
Environment variables.
Secrets access.
CPU limits.
Memory limits.
Timeouts.
Process isolation.

## 22.3 Secret Access

Plugins should never receive all secrets by default.

Secrets should be scoped.

Example:

GitHub plugin gets GitHub token.

GitHub plugin does not get:

OpenAI key.
Database password.
Slack token.
Company secrets.

## 22.4 Plugin Permissions

Plugins themselves may need permissions.

Example:

A ToolSet plugin may need permission to call external network.

A ServiceProvider plugin may need permission to write local files.

An ExecutionEnvironment plugin may need permission to run containers.

## 22.5 Plugin Verification

The registry should support:

Checksums.
Signatures.
Publisher identity.
Version history.
Security warnings.
Dependency scan.
Compatibility checks.

---

# 23. Failure Isolation

## 23.1 Failure Philosophy

Relux should be designed so that one broken component does not destroy the whole system.

A plugin can fail.
An agent can fail.
A task can fail.
A tool call can fail.
A model provider can fail.
A database can be unavailable.

The kernel should stay alive.

## 23.2 Plugin Failure

If a plugin fails:

Mark plugin unhealthy.
Stop routing new requests to it.
Finish or fail active requests safely.
Notify operators.
Retry health checks.
Allow restart.
Record audit log.

## 23.3 Agent Failure

If an agent fails:

Expire lease.
Mark task recoverable if possible.
Reassign task if allowed.
Keep logs.
Notify operator if needed.

## 23.4 Tool Call Failure

If a tool call fails:

Return structured error to agent.
Allow retry if safe.
Do not repeat unsafe actions automatically.
Log failure.
Escalate if needed.

## 23.5 Model Provider Failure

If a model provider fails:

Adapter reports model_error.
Kernel can retry if safe.
Kernel can route to fallback adapter if configured.

Example fallback:

```yaml
agents:
  - name: "research-agent"
    adapter:
      plugin: relix-adapter-openai
      fallback:
        plugin: relix-adapter-anthropic
```

---

# 24. Observability

## 24.1 What Relux Should Track

Task metrics:

Created.
Completed.
Failed.
Duration.
Retries.
Agent assignment.

Tool metrics:

Tool calls.
Latency.
Failure rate.
Approval rate.
Denied calls.

Agent metrics:

Tasks completed.
Success rate.
Average duration.
Tool usage.
Cost.
Error rate.

Plugin metrics:

Health.
Latency.
Failures.
Restarts.
Version.

Security metrics:

Permission denials.
Approval requests.
High-risk actions.
Rejected actions.
Secret access.

## 24.2 Trace Example

A trace should show:

```text
Task: Fix failing test
  Agent: code-agent
    Tool: github.read_repo
    Tool: terminal.run_tests
    Tool: github.create_branch
    Tool: github.create_pr
  Result: completed
```

## 24.3 Logs

Logs should be structured.

Example:

```json
{
  "timestamp": "2026-06-08T12:00:00Z",
  "level": "info",
  "event": "tool_call.started",
  "task_id": "task_123",
  "agent_id": "agent_code",
  "plugin": "relix-tools-github",
  "tool": "github.create_pr"
}
```

---

# 25. Example Use Cases

## 25.1 Use Case: Coding Agent

Goal:

A developer wants an agent that can fix bugs and open PRs.

Plugins:

relix-adapter-anthropic
relix-tools-github
relix-tools-terminal
relix-env-python-wasm
relix-provider-sqlite

Permissions:

```text
tool:relix-tools-github:read_repo
tool:relix-tools-github:create_branch
tool:relix-tools-github:create_pr
tool:relix-tools-terminal:run_tests
exec:relix-env-python-wasm:run
```

Denied:

```text
tool:relix-tools-github:merge_pr
tool:relix-tools-github:delete_repo
exec:relix-env-shell:run_root
```

Flow:

```text
User creates task.
Code agent reads repo.
Code agent runs tests.
Code agent edits files.
Code agent creates branch.
Code agent opens PR.
Task completes.
Human reviews PR.
```

## 25.2 Use Case: Support Agent

Goal:

A company wants an agent that drafts customer support replies.

Plugins:

relix-adapter-openai
relix-tools-zendesk
relix-tools-slack
relix-tools-policy-search
relix-memory-qdrant

Permissions:

```text
tool:relix-tools-zendesk:read_ticket
tool:relix-tools-zendesk:draft_reply
tool:relix-tools-policy-search:search
tool:relix-tools-slack:send_internal
```

Requires approval:

```text
tool:relix-tools-zendesk:send_reply
tool:relix-tools-stripe:issue_refund
```

Flow:

```text
Ticket arrives.
Support agent reads ticket.
Agent searches policy.
Agent drafts reply.
If refund needed, approval request is created.
Human approves or rejects.
Agent completes ticket.
```

## 25.3 Use Case: Enterprise Internal Agent Network

Goal:

A company wants multiple agents working together.

Agents:

Manager agent.
Research agent.
Code agent.
Legal agent.
Support agent.
Deployment agent.

The manager agent breaks down work.

Example request:

```text
Prepare a new customer onboarding automation.
```

Subtasks:

Research agent studies current onboarding docs.
Code agent builds integration.
Legal agent checks privacy language.
Support agent drafts help center article.
Deployment agent prepares staging deployment.

Relux manages permissions, tasks, logs, approvals, and tool routing.

## 25.4 Use Case: SOL Runtime

Goal:

A developer wants to run SOL workflows inside Relux.

Plugins:

relix-env-sol
relix-tools-github
relix-tools-terminal
relix-provider-postgres

Flow:

```text
User installs SOL runtime.
User writes workflow.sol.
Relux runs workflow inside a session.
Each SOL process becomes traceable.
Tools are called through permissioned plugins.
Output is logged.
```

## 25.5 Use Case: Private Company Plugin

Goal:

A company wants agents to access an internal CRM.

They build:

```text
relix-tools-acme-crm
```

Tools:

```text
crm.search_customer
crm.read_account
crm.create_note
crm.update_status
```

Permissions:

```text
tool:relix-tools-acme-crm:search_customer
tool:relix-tools-acme-crm:read_account
tool:relix-tools-acme-crm:create_note
```

High-risk action:

```text
tool:relix-tools-acme-crm:update_status
```

This may require approval.

---

# 26. Product Requirements

## 26.1 Must Have for MVP

Kernel can start.
Kernel can load plugins.
SQLite provider works.
Plugin manifest validation works.
CLI can install plugin.
CLI can create company.
CLI can create agent.
OpenAI or Anthropic adapter works.
One ToolSet plugin works.
Agent can run a task.
Agent can call tool through kernel.
Permission check works.
Audit log records actions.
Dashboard shows installed plugins.
Dashboard shows agents.
Dashboard shows tasks.

## 26.2 Should Have

Approval flow.
Plugin health checks.
WebSocket task updates.
Task leases.
Retry behavior.
Plugin disable/enable.
Basic role management.
Basic namespace support.
Config through relix.yaml.
Logs page.

## 26.3 Could Have

Plugin marketplace.
WASM sandboxing.
Private registries.
OpenTelemetry export.
Policy engine integration.
Multiple model fallback.
UI extension plugins.
Automated SDK generation.
Advanced memory permissions.

---

# 27. Non-Goals for MVP

Relux should not try to build every plugin immediately.

Do not build every possible integration in MVP.

Do not build a full marketplace first.

Do not build complex enterprise compliance first.

Do not support every runtime first.

Do not build advanced visual workflow editor first.

Do not over-engineer distributed clustering before the basic loop works.

The MVP should prove:

```text
Kernel loads plugin.
Agent runs task.
Agent calls tool.
Kernel checks permission.
Tool executes.
Action is logged.
Dashboard shows it.
```

That loop matters most.

---

# 28. Implementation Phases

## 28.1 Phase 0: Blueprint

Goal:

Finalize architecture.

Deliverables:

Detailed spec.
Plugin manifest schema.
Core trait definitions.
Entity model.
Permission format.
Basic API design.

Success:

Team agrees on what Relux is.

## 28.2 Phase 1: Kernel Foundation

Goal:

Build minimal kernel.

Deliverables:

relix-kernel binary.
Plugin manifest loader.
Plugin registry local index.
SQLite PrimaryStorage provider.
Basic CLI.
Company creation.
Agent creation.
Permission storage.
Audit log storage.

Success:

You can configure a Relux system, but agents do not need to run yet.

## 28.3 Phase 2: First Agent Loop

Goal:

Make one agent run one task and use one tool.

Deliverables:

First Adapter plugin.
First ToolSet plugin.
Task creation.
Task leasing.
Tool call routing.
Permission check.
Audit logging.
Integration test.

Success:

A user can create a task and watch an agent call a tool through Relux.

## 28.4 Phase 3: Dashboard MVP

Goal:

Give operators visibility.

Deliverables:

Dashboard home.
Plugin list.
Agent list.
Task list.
Task detail.
Tool call logs.
Permission viewer.

Success:

A user can understand what the system is doing without reading terminal logs.

## 28.5 Phase 4: Security and Approval

Goal:

Make actions safe.

Deliverables:

Risk levels.
Approval requests.
Approval dashboard.
Permission templates.
Human approval flow.
Denied action logs.

Success:

High-risk tool calls can be blocked until approved.

## 28.6 Phase 5: Plugin Ecosystem

Goal:

Make Relux extensible by others.

Deliverables:

Plugin SDK.
Plugin registry.
Plugin templates.
Plugin signing.
Official plugin examples.
Private plugin support.

Success:

A third-party developer can build, install, and run a plugin.

## 28.7 Phase 6: Scale and Reliability

Goal:

Make Relux production-ready.

Deliverables:

Task broker provider.
Horizontal worker support.
Plugin isolation.
Retry policies.
Fallback adapters.
OpenTelemetry.
Backup and restore.
Namespace scaling.

Success:

Relux can support real multi-agent workloads.

---

# 29. Suggested Repository Structure

```text
relix/
  crates/
    relix-kernel/
    relix-core/
    relix-plugin-api/
    relix-plugin-host/
    relix-storage/
    relix-auth/
    relix-router/
    relix-task/
    relix-audit/
    relix-cli/

  plugins/
    relix-provider-sqlite/
    relix-provider-postgres/
    relix-adapter-openai/
    relix-adapter-anthropic/
    relix-tools-terminal/
    relix-tools-github/
    relix-env-python-wasm/
    relix-env-sol/

  dashboard/
    app/
    components/
    pages/
    lib/

  sdk/
    typescript/
    python/
    rust/

  examples/
    coding-agent/
    support-agent/
    sol-runtime/
    private-crm-plugin/

  docs/
    architecture.md
    plugin-system.md
    permissions.md
    task-orchestration.md
    dashboard.md
    cli.md
```

---

# 30. Example End-to-End Scenario

## Scenario: Build a Coding Agent

### Step 1: Initialize Relux

```bash
relix init
```

### Step 2: Install Plugins

```bash
relix plugins install relix-adapter-anthropic
relix plugins install relix-tools-github
relix plugins install relix-tools-terminal
relix plugins install relix-env-python-wasm
```

### Step 3: Configure Plugins

```bash
relix plugins configure relix-adapter-anthropic
relix plugins configure relix-tools-github
```

### Step 4: Create Agent

```bash
relix agents create code-agent \
  --adapter relix-adapter-anthropic \
  --model claude-sonnet
```

### Step 5: Grant Permissions

```bash
relix permissions grant code-agent tool:relix-tools-github:read_repo
relix permissions grant code-agent tool:relix-tools-github:create_branch
relix permissions grant code-agent tool:relix-tools-github:create_pr
relix permissions grant code-agent tool:relix-tools-terminal:run_tests
relix permissions grant code-agent exec:relix-env-python-wasm:run
```

### Step 6: Run Task

```bash
relix tasks create \
  --agent code-agent \
  --title "Fix failing invoice test" \
  --input "Run the billing tests, fix the failing invoice test, and open a PR."
```

### Step 7: Watch Task

```bash
relix tasks watch task_123
```

### Step 8: Dashboard Output

Dashboard shows:

```text
Task: Fix failing invoice test
Status: completed
Agent: code-agent
Tools used:
- github.read_repo
- terminal.run_tests
- github.create_branch
- github.create_pr
Output:
PR created: https://github.com/acme/billing/pull/42
```

---

# 31. Key Differentiators

## 31.1 Relux Is Not an Agent, It Is the Control Plane

Most products focus on the agent.

Relux focuses on the infrastructure around agents.

## 31.2 Everything Is Replaceable

Models are replaceable.
Tools are replaceable.
Storage is replaceable.
Memory is replaceable.
Runtimes are replaceable.
Task brokers are replaceable.

## 31.3 Permissions Are Core, Not an Afterthought

Every agent action should go through Relux permissioning.

## 31.4 Plugin Ecosystem

Relux grows through plugins, not kernel bloat.

## 31.5 Designed for Real Companies

Relux includes:

Namespaces.
Users.
Roles.
Approvals.
Audit logs.
Plugin health.
Task visibility.
Secrets.
Observability.

## 31.6 Failure Isolation

One plugin failure should not collapse the whole system.

---

# 32. MVP Success Criteria

The MVP is successful if a user can:

Install Relux.
Install an adapter plugin.
Install a tool plugin.
Create an agent.
Grant permissions.
Create a task.
Watch the agent run.
See a tool call routed through the kernel.
See permission enforcement.
See audit logs.
View results in dashboard.

The most important demo:

```text
An agent receives a task, asks to use a tool, the kernel checks permission, routes the call to a plugin, receives the result, logs everything, and shows it in the dashboard.
```

That proves the whole Relux idea.

---

# 33. Future Vision

Long term, Relux should become the standard operating layer for agentic applications.

A developer building an AI product should not ask:

```text
How do I rebuild auth, permissions, storage, tools, memory, and task routing?
```

They should ask:

```text
Which Relux plugins do I need?
```

A company deploying agents should not ask:

```text
How do I know what this agent can access?
```

They should open Relux and see:

```text
This agent can read tickets.
This agent can draft replies.
This agent cannot send replies without approval.
This agent cannot issue refunds.
This agent used these tools.
This agent completed these tasks.
This agent failed here.
This human approved this action.
```

The future of software will have many agents. Relux is the system that makes them manageable.

The final vision:

Relux is the control plane for the agentic software stack.

Everything is a plugin.
Every agent is permissioned.
Every action is routed.
Every task is observable.
Every system can connect.
Every company can build its own agentic platform.
