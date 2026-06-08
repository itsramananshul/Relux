# Relix: The Control Plane for Agentic Applications - Detailed Specification

*Version: 0.2.0*
*Status: DRAFT (Plugin-First Architecture)*

## 1. Core Philosophy: Everything is a Plugin

Relix is not a monolithic application; it is a lightweight **Plugin Kernel**. Its sole purpose is to discover, load, configure, and orchestrate a universe of plugins. Every piece of functionality—from agent capabilities and toolsets to core infrastructure backends like databases and memory stores—is implemented as a plugin.

This "everything is a plugin" model provides maximum extensibility and user choice. If a new technology, agent, or tool appears, it can be integrated into any Relix system by anyone, without requiring changes to the core kernel. The user has ultimate control to assemble the exact platform they need.

**The "Boom, Plugin" Experience:**
1.  **Discover:** `relix plugins search <keyword>`
2.  **Install:** `relix plugins install <plugin_name>`
3.  **Configure:** Activate and provide credentials in the Dashboard or a `relix.yaml` file.
4.  **Use:** The new capability is immediately available to authorized agents.

---

## 2. The Plugin Kernel Architecture

The Relix Control Plane is a minimal core responsible for four things:
1.  **Plugin Lifecycle Management:** Installing, upgrading, and managing plugin packages.
2.  **State Management:** Persisting the core entities (Agent, Task, Company, etc.) using a configurable `PrimaryStorage` provider plugin.
3.  **Security Enforcement:** Authenticating and authorizing every API call against a central RBAC model.
4.  **Request Routing:** Directing incoming API calls (from users or agents) to the appropriate destination plugin.

The kernel itself contains no business logic for specific agent types, tools, or infrastructure. It is programmed against abstract interfaces (Rust traits) that are implemented by plugins.

```
+---------------------------------+
|      Relix Plugin Kernel        |
|---------------------------------|
|  - Plugin Lifecycle             |
|  - State Management (Core Nouns)|
|  - Security (AuthN/AuthZ)       |
|  - API Routing (REST/gRPC)      |
+---------------------------------+
     ^           ^           ^
     |           |           |
+----|-----------|-----------|----+
|    v           v           v    |
|      PLUGIN HOST & REGISTRY     |
|                                 |
+---------------------------------+
     ^           ^           ^
     |           |           |
[ Adapter ] [ Service ] [ ToolSet ] [ Env ]
[ Plugin  ] [ Provider] [ Plugin  ] [Plugin] ...
```

---

## 3. Plugin Extension Points

A plugin is a self-contained package that implements one or more standard interfaces, known as "Extension Points."

### 3.1. `Adapter` Extension
*   **Purpose:** Provides the logic for a specific *type* of agent. This is how you integrate new agent applications (like Claude, Hermes, or custom-built ones).
*   **Interface:** An Adapter must register the types of tasks it can handle and expose methods for executing them.
*   **Examples:** `relix-adapter-openai`, `relix-adapter-anthropic`, `relix-adapter-hermes`, `relix-adapter-ollama`.

### 3.2. `ServiceProvider` Extension
*   **Purpose:** Provides an implementation for a core infrastructure backend. This is how users choose their own database, vector store, or task broker.
*   **Interface:** Implements a specific `trait` from the kernel, such as `trait PrimaryStorage`, `trait VectorStore`, or `trait TaskBroker`.
*   **Examples:**
    *   **Vector Store:** `relix-provider-qdrant`, `relix-provider-chromadb`.
    *   **Task Broker:** `relix-provider-redis`, `relix-provider-nats`.
    *   **Primary Storage:** `relix-provider-postgres`, `relix-provider-sqlite`.

### 3.3. `ToolSet` Extension
*   **Purpose:** Adds a collection of new, usable functions (tools) that can be granted to any agent.
*   **Interface:** A ToolSet plugin registers a schema of available functions (e.g., an OpenAPI spec) and provides an endpoint to execute them.
*   **Examples:** `relix-tools-github` (provides `github.create_pr`), `relix-tools-tavily`, `relix-tools-discord`.

### 3.4. `ExecutionEnvironment` Extension
*   **Purpose:** Provides a sandboxed runtime for executing code or specific program types.
*   **Interface:** Exposes a `run` method that takes a code payload and returns the result. It registers the languages/formats it can execute (e.g., `python`, `javascript`, `.sol`).
*   **Examples:** `relix-env-python-wasm`, `relix-env-docker`, `relix-env-sol` (for running Soul code).

---

## 4. Configuration: The `relix.yaml` File

The user configures the entire system through a single file, which declares which plugins to use for which function and provides the necessary configuration.

```yaml
# relix.yaml - Example Configuration

company_id: "acme-corp"

# Override core service providers. If omitted, uses built-in defaults (e.g., sqlite).
providers:
  primary_storage:
    plugin: relix-provider-postgres
    config:
      connection_string: "${POSTGRES_URL}"

  vector_store:
    plugin: relix-provider-qdrant
    config:
      url: "http://localhost:6333"

# Define the agents and the plugins that power them.
agents:
  - name: "research-assistant"
    adapter:
      plugin: relix-adapter-openai
      config:
        model: "gpt-4o"
        api_key: "${OPENAI_API_KEY}"
    # Grant this agent permissions to use tools from a specific plugin.
    permissions:
      - "tool:relix-tools-tavily:search"

  - name: "code-monkey"
    adapter:
      plugin: relix-adapter-anthropic
      config:
        model: "claude-3-opus-20240229"
        api_key: "${ANTHROPIC_API_KEY}"
    permissions:
      - "tool:relix-tools-github:*" # Can use all tools from the GitHub plugin
      - "exec:relix-env-python-wasm:run" # Can execute python code
```
---

## 5. Core Entities (Managed by the Kernel)

These are the primary data models the kernel manages, stored using the configured `PrimaryStorage` provider.

*   **Company, Namespace, User, Agent:** The core identity and tenancy models.
*   **Task, Lease:** The core work orchestration models.
*   **PluginRegistration:** A record of every installed plugin, its version, and its health.
*   **Role, Permission:** The data for the RBAC system. Permissions are strings that link to plugins, e.g., `tool:<plugin_name>:<tool_name>`.
*   **Audit Log:** An immutable log of all significant actions.

---

## 6. API & Protocols

The kernel exposes a unified API for all interactions.

*   **REST API (for Management & Compatibility):** Used by the Dashboard and operators to manage the system (install plugins, create agents, etc.). Defined in `openapi.yaml`.
*   **gRPC API (for Performance):** Used by agents and plugins for high-throughput communication (e.g., task streaming, tool calls). Defined in `protos/`.
*   **WebSockets (for Events):** Pushes real-time status updates to subscribed clients.

---

## 7. Phased Implementation Plan (Plugin-First)

*   **Phase 0: The Kernel Blueprint (Current Stage)**
    *   **Goal:** Finalize the Plugin-First design.
    *   **Deliverables:** This document, initial API/trait definitions for the kernel.

*   **Phase 1: The Plugin Kernel & First Service Provider**
    *   **Goal:** Build the minimum viable kernel and the default storage plugin.
    *   **Deliverables:**
        *   `relix-kernel`: The main binary that can load a plugin.
        *   `relix-provider-sqlite`: The first `ServiceProvider` plugin, implementing the `PrimaryStorage` trait.
        *   `relix-cli`: A basic CLI for installing a plugin and creating a Company/Agent.
        *   **No agents can run yet.** The system can only be configured.

*   **Phase 2: The First Adapter & Tool**
    *   **Goal:** Enable an agent to run and use one tool.
    *   **Deliverables:**
        *   `relix-adapter-openai`: The first `Adapter` plugin.
        *   `relix-tools-terminal`: The first `ToolSet` plugin.
        *   The kernel can now dispatch a `Task` to the OpenAI adapter, which can then make a `Tool Call` back to the kernel, which routes it to the terminal plugin.
        *   A single integration test proving the entire loop works.

*   **Phase 3: The Operator's View**
    *   **Goal:** Provide a UI for managing the plugin-based system.
    *   **Deliverables:**
        *   A Dashboard UI that can list installed plugins from the kernel's API.
        *   UI for creating agents and selecting their `Adapter` plugin from a dropdown of installed adapters.
        *   UI for assigning permissions to `ToolSet` plugins.

*   **Phase 4 & Beyond: The Ecosystem Explosion**
    *   Develop a rich ecosystem of official and community plugins for all extension points (`ServiceProvider`, `Adapter`, `ToolSet`, `ExecutionEnvironment`).
    *   Build a public or private **Plugin Registry**.
    *   Add features like plugin sandboxing (WASM), dependency management, and automated SDK generation.
