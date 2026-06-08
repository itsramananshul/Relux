//! The in-memory control-plane state and the kernel actions that mutate it.
//!
//! `KernelState` is the local, deterministic core of Relux: it stores plugin
//! manifests, namespaces, agents, tasks, runs, run transcripts, and the audit
//! log, and exposes the minimal set of kernel actions needed to prove the first
//! loop from `docs/RELUX_MASTER_PLAN.md` section 14 (MVP Definition):
//!
//! ```text
//! create namespace -> create agent -> create task -> start run ->
//! call tool (permission-checked) -> complete run -> complete task
//! ```
//!
//! Every meaningful action routes through here and records an audit event, so
//! nothing bypasses the kernel (section 17.5, section 10.2). No storage, no network - this is
//! the seam a SQLite/Postgres ServiceProvider plugin will later sit behind.

use std::collections::HashMap;

use relux_core::agent::AgentStatus;
use relux_core::namespace::NamespaceKind;
use relux_core::{
    Agent, AgentId, AuditEvent, AuditResult, Namespace, NamespaceId, Permission, PluginId,
    PluginManifest, Run, RunId, RunStatus, Task, TaskId, TaskStatus,
};

use crate::clock::Clock;
use crate::event::RunEvent;
use crate::KernelError;

/// The local, in-memory Relux control plane.
#[derive(Debug, Default)]
pub struct KernelState {
    plugins: HashMap<PluginId, PluginManifest>,
    namespaces: HashMap<NamespaceId, Namespace>,
    agents: HashMap<AgentId, Agent>,
    tasks: HashMap<TaskId, Task>,
    runs: HashMap<RunId, Run>,
    /// Per-run transcripts, in emission order.
    run_events: Vec<RunEvent>,
    /// The append-only audit log, in emission order.
    audit_log: Vec<AuditEvent>,

    clock: Clock,
    next_task: u64,
    next_run: u64,
    next_audit: u64,
    next_event: u64,
}

impl KernelState {
    pub fn new() -> Self {
        Self::default()
    }

    // --- Plugins -----------------------------------------------------------

    /// Register an already-validated plugin manifest into the local index.
    ///
    /// Manifests are expected to have passed `relux_core::validate_manifest`
    /// (the loader enforces this); registration records an audit event.
    pub fn register_plugin(&mut self, manifest: PluginManifest) {
        let id = manifest.id.clone();
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:register",
            Some("plugin"),
            Some(id.as_str()),
            None,
            AuditResult::Success,
            serde_json::json!({ "kind": format!("{:?}", manifest.kind) }),
        );
        self.plugins.insert(id, manifest);
    }

    pub fn plugin(&self, id: &PluginId) -> Option<&PluginManifest> {
        self.plugins.get(id)
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    // --- Namespaces --------------------------------------------------------

    /// Create an isolation scope (`docs/RELUX_MASTER_PLAN.md` section 9.2).
    pub fn create_namespace(&mut self, id: &str, name: &str, kind: NamespaceKind) -> NamespaceId {
        let ns_id = NamespaceId::new(id);
        let namespace = Namespace {
            id: ns_id.clone(),
            name: name.to_string(),
            kind,
            parent_id: None,
            settings: serde_json::Value::Null,
            created_at: self.clock.tick(),
        };
        self.record_audit(
            "kernel",
            "kernel",
            "namespace:create",
            Some("namespace"),
            Some(ns_id.as_str()),
            Some(&ns_id),
            AuditResult::Success,
            serde_json::Value::Null,
        );
        self.namespaces.insert(ns_id.clone(), namespace);
        ns_id
    }

    pub fn namespace_count(&self) -> usize {
        self.namespaces.len()
    }

    // --- Agents ------------------------------------------------------------

    /// Create a configured agent actor (`docs/RELUX_MASTER_PLAN.md` section 9.3).
    ///
    /// `adapter_plugin` must reference a registered Adapter plugin; the agent is
    /// granted exactly `permissions` and nothing more (least privilege, section 17.5).
    #[allow(clippy::too_many_arguments)]
    pub fn create_agent(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        adapter_plugin: &PluginId,
        namespace: &NamespaceId,
        persona: Option<String>,
        permissions: Vec<Permission>,
    ) -> Result<AgentId, KernelError> {
        if !self.plugins.contains_key(adapter_plugin) {
            return Err(KernelError::UnknownPlugin(adapter_plugin.to_string()));
        }
        let agent_id = AgentId::new(id);
        let agent = Agent {
            id: agent_id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            adapter_plugin: adapter_plugin.clone(),
            adapter_config: serde_json::Value::Null,
            persona,
            namespace_id: namespace.clone(),
            owner: "founder".to_string(),
            permissions,
            status: AgentStatus::Active,
            created_at: self.clock.tick(),
        };
        self.record_audit(
            "kernel",
            "kernel",
            "agent:create",
            Some("agent"),
            Some(agent_id.as_str()),
            Some(namespace),
            AuditResult::Success,
            serde_json::json!({ "adapter": adapter_plugin.as_str() }),
        );
        self.agents.insert(agent_id.clone(), agent);
        Ok(agent_id)
    }

    pub fn agent(&self, id: &AgentId) -> Option<&Agent> {
        self.agents.get(id)
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    // --- Tasks -------------------------------------------------------------

    /// Create a durable unit of work (`docs/RELUX_MASTER_PLAN.md` section 9.5).
    pub fn create_task(
        &mut self,
        title: &str,
        input: serde_json::Value,
        created_by: &str,
        namespace: &NamespaceId,
        required_permissions: Vec<Permission>,
    ) -> TaskId {
        self.next_task += 1;
        let task_id = TaskId::new(format!("task_{:04}", self.next_task));
        let now = self.clock.tick();
        let task = Task {
            id: task_id.clone(),
            title: title.to_string(),
            input,
            status: TaskStatus::Created,
            priority: 5,
            created_by: created_by.to_string(),
            assigned_agent: None,
            namespace_id: namespace.clone(),
            required_permissions,
            parent_task: None,
            deadline: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.record_audit(
            created_by_actor(created_by).0,
            created_by_actor(created_by).1,
            "task:create",
            Some("task"),
            Some(task_id.as_str()),
            Some(namespace),
            AuditResult::Success,
            serde_json::json!({ "title": title }),
        );
        self.tasks.insert(task_id.clone(), task);
        task_id
    }

    /// Assign a task to an agent and move it to `Queued`
    /// (`docs/RELUX_MASTER_PLAN.md` section 13.3).
    pub fn assign_task(&mut self, task_id: &TaskId, agent_id: &AgentId) -> Result<(), KernelError> {
        if !self.agents.contains_key(agent_id) {
            return Err(KernelError::UnknownAgent(agent_id.to_string()));
        }
        let now = self.clock.tick();
        let namespace = {
            let task = self
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            task.assigned_agent = Some(agent_id.clone());
            task.status = TaskStatus::Queued;
            task.updated_at = now;
            task.namespace_id.clone()
        };
        self.record_audit(
            "kernel",
            "kernel",
            "task:assign",
            Some("task"),
            Some(task_id.as_str()),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({ "agent": agent_id.as_str() }),
        );
        Ok(())
    }

    pub fn task(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.get(id)
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    // --- Runs --------------------------------------------------------------

    /// Start an execution attempt for an assigned task
    /// (`docs/RELUX_MASTER_PLAN.md` section 9.6, section 13.6). The run inherits the assigned
    /// agent's adapter plugin and the task moves to `Running`.
    pub fn start_run(&mut self, task_id: &TaskId) -> Result<RunId, KernelError> {
        let (agent_id, namespace) = {
            let task = self
                .tasks
                .get(task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            let agent_id = task
                .assigned_agent
                .clone()
                .ok_or_else(|| KernelError::TaskNotAssigned(task_id.to_string()))?;
            (agent_id, task.namespace_id.clone())
        };
        let adapter_plugin = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?
            .adapter_plugin
            .clone();

        self.next_run += 1;
        let run_id = RunId::new(format!("run_{:04}", self.next_run));
        let started = self.clock.tick();
        let run = Run {
            id: run_id.clone(),
            task_id: task_id.clone(),
            agent_id: agent_id.clone(),
            adapter_plugin: adapter_plugin.clone(),
            status: RunStatus::Running,
            started_at: Some(started),
            ended_at: None,
            summary: None,
            error: None,
        };
        self.runs.insert(run_id.clone(), run);

        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = TaskStatus::Running;
            task.updated_at = self.clock.tick();
        }

        self.push_run_event(
            &run_id,
            "run_started",
            "kernel",
            &format!("run started for task {task_id} on adapter {adapter_plugin}"),
            serde_json::json!({ "agent": agent_id.as_str(), "adapter": adapter_plugin.as_str() }),
        );
        self.record_audit(
            "agent",
            agent_id.as_str(),
            "task:start_run",
            Some("run"),
            Some(run_id.as_str()),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({ "task": task_id.as_str() }),
        );
        Ok(run_id)
    }

    /// Route a tool call from an agent through the kernel
    /// (`docs/RELUX_MASTER_PLAN.md` section 13.6, section 10.2).
    ///
    /// The kernel resolves the tool on the named plugin, looks up the permission
    /// it requires, and verifies the agent holds it. Denials and successes are
    /// both audited and recorded on the run transcript. The echo tool returns its
    /// input unchanged - proof of the loop without any external effect.
    pub fn call_tool(
        &mut self,
        run_id: &RunId,
        agent_id: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, KernelError> {
        let namespace = self
            .agents
            .get(agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?
            .namespace_id
            .clone();

        let manifest = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| KernelError::UnknownPlugin(plugin_id.to_string()))?;
        let tool = manifest
            .capabilities
            .tools
            .iter()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| KernelError::ToolNotFound {
                plugin: plugin_id.to_string(),
                tool: tool_name.to_string(),
            })?;
        let required = tool.permission.clone();

        let agent = self
            .agents
            .get(agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?;
        let allowed = agent.permissions.iter().any(|p| p.matches_exact(&required));

        if !allowed {
            self.push_run_event(
                run_id,
                "tool_call_denied",
                "kernel",
                &format!("denied {tool_name}: agent lacks {required}"),
                serde_json::json!({ "tool": tool_name, "permission": required.as_str() }),
            );
            self.record_audit(
                "agent",
                agent_id.as_str(),
                required.as_str(),
                Some("tool"),
                Some(tool_name),
                Some(&namespace),
                AuditResult::Denied,
                serde_json::json!({ "run": run_id.as_str() }),
            );
            return Err(KernelError::PermissionDenied {
                agent: agent_id.to_string(),
                permission: required.to_string(),
            });
        }

        // The echo tool is the whole "plugin": it returns its input unchanged.
        let output = input.clone();
        self.push_run_event(
            run_id,
            "tool_call",
            agent_id.as_str(),
            &format!("called {tool_name} via {plugin_id}"),
            serde_json::json!({ "tool": tool_name, "input": input, "output": output }),
        );
        self.record_audit(
            "agent",
            agent_id.as_str(),
            required.as_str(),
            Some("tool"),
            Some(tool_name),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({ "run": run_id.as_str() }),
        );
        Ok(output)
    }

    /// Mark a run completed with a summary (`docs/RELUX_MASTER_PLAN.md` section 9.6).
    pub fn complete_run(&mut self, run_id: &RunId, summary: &str) -> Result<(), KernelError> {
        let ended = self.clock.tick();
        let (agent_id, task_id) = {
            let run = self
                .runs
                .get_mut(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            run.status = RunStatus::Completed;
            run.ended_at = Some(ended);
            run.summary = Some(summary.to_string());
            (run.agent_id.clone(), run.task_id.clone())
        };
        let task_namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());
        self.push_run_event(
            run_id,
            "run_completed",
            "kernel",
            summary,
            serde_json::Value::Null,
        );
        self.record_audit(
            "agent",
            agent_id.as_str(),
            "run:complete",
            Some("run"),
            Some(run_id.as_str()),
            task_namespace.as_ref(),
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(())
    }

    /// Mark a task completed (`docs/RELUX_MASTER_PLAN.md` section 9.5).
    pub fn complete_task(&mut self, task_id: &TaskId) -> Result<(), KernelError> {
        let now = self.clock.tick();
        let namespace = {
            let task = self
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            task.status = TaskStatus::Completed;
            task.updated_at = now;
            task.namespace_id.clone()
        };
        self.record_audit(
            "kernel",
            "kernel",
            "task:complete",
            Some("task"),
            Some(task_id.as_str()),
            Some(&namespace),
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(())
    }

    // --- Inspection --------------------------------------------------------

    pub fn run(&self, id: &RunId) -> Option<&Run> {
        self.runs.get(id)
    }

    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// The transcript for one run, in emission order.
    pub fn run_events(&self, run_id: &RunId) -> Vec<&RunEvent> {
        self.run_events
            .iter()
            .filter(|e| &e.run_id == run_id)
            .collect()
    }

    /// The full append-only audit log, in emission order.
    pub fn audit_log(&self) -> &[AuditEvent] {
        &self.audit_log
    }

    // --- Internal ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn record_audit(
        &mut self,
        actor_type: &str,
        actor_id: &str,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        namespace: Option<&NamespaceId>,
        result: AuditResult,
        metadata: serde_json::Value,
    ) {
        self.next_audit += 1;
        let event = AuditEvent {
            id: format!("audit_{:04}", self.next_audit),
            ts: self.clock.tick(),
            actor_type: actor_type.to_string(),
            actor_id: actor_id.to_string(),
            action: action.to_string(),
            target_type: target_type.map(str::to_string),
            target_id: target_id.map(str::to_string),
            namespace_id: namespace.cloned(),
            result,
            metadata,
        };
        self.audit_log.push(event);
    }

    fn push_run_event(
        &mut self,
        run_id: &RunId,
        kind: &str,
        source: &str,
        message: &str,
        payload: serde_json::Value,
    ) {
        self.next_event += 1;
        self.run_events.push(RunEvent {
            id: format!("revent_{:04}", self.next_event),
            run_id: run_id.clone(),
            ts: self.clock.tick(),
            kind: kind.to_string(),
            source: source.to_string(),
            message: message.to_string(),
            payload,
        });
    }
}

/// Map a `created_by` string onto an `(actor_type, actor_id)` pair for the audit
/// log. Anything that is not a bare human handle is treated as an agent actor.
fn created_by_actor(created_by: &str) -> (&str, &str) {
    if created_by == "founder" || created_by == "user" {
        ("user", created_by)
    } else {
        ("agent", created_by)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::permission::{ApprovalRequirement, RiskLevel, ToolDefinition};
    use relux_core::{PluginCapability, PluginHealth, PluginKind, TrustLevel};

    fn echo_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-tools-echo"),
            name: "Echo".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "echo".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![ToolDefinition {
                    name: "echo.say".to_string(),
                    description: "echoes".to_string(),
                    risk: RiskLevel::Low,
                    permission: Permission::new("tool:relux-tools-echo:say").unwrap(),
                    approval: ApprovalRequirement::Never,
                    timeout_secs: Some(5),
                }],
                permissions: vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
            },
            health: PluginHealth::Unknown,
        }
    }

    fn adapter_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-adapter-local-prime"),
            name: "Local Prime".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::Adapter,
            description: "adapter".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![],
                permissions: vec![Permission::new("adapter:relux-adapter-local-prime:run").unwrap()],
            },
            health: PluginHealth::Unknown,
        }
    }

    /// Build a kernel wound up to an in-progress run by Prime, returning the ids.
    fn primed_kernel() -> (KernelState, AgentId, TaskId, RunId, PluginId) {
        let mut k = KernelState::new();
        k.register_plugin(echo_manifest());
        k.register_plugin(adapter_manifest());
        let ns = k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
        let echo = PluginId::new("relux-tools-echo");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let prime = k
            .create_agent(
                "prime",
                "Prime",
                "The control-plane operator.",
                &adapter,
                &ns,
                None,
                vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
            )
            .unwrap();
        let task = k.create_task(
            "Check the echo tool responds",
            serde_json::json!({ "message": "hello relux" }),
            "founder",
            &ns,
            vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
        );
        k.assign_task(&task, &prime).unwrap();
        let run = k.start_run(&task).unwrap();
        (k, prime, task, run, echo)
    }

    #[test]
    fn full_loop_completes_and_audits() {
        let (mut k, prime, task, run, echo) = primed_kernel();

        let out = k
            .call_tool(
                &run,
                &prime,
                &echo,
                "echo.say",
                serde_json::json!({ "message": "hello relux" }),
            )
            .expect("tool call allowed");
        assert_eq!(out, serde_json::json!({ "message": "hello relux" }));

        k.complete_run(&run, "echo returned input").unwrap();
        k.complete_task(&task).unwrap();

        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Completed);
        assert_eq!(k.run(&run).unwrap().status, RunStatus::Completed);

        // The transcript shows start -> tool_call -> completed.
        let kinds: Vec<&str> = k.run_events(&run).iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, vec!["run_started", "tool_call", "run_completed"]);

        // The tool call was audited as a success.
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "tool:relux-tools-echo:say"
                && e.result == AuditResult::Success));
    }

    #[test]
    fn unpermissioned_tool_call_is_denied_and_audited() {
        let (mut k, _prime, _task, run, echo) = primed_kernel();

        // A second agent with no tool permissions.
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let weak = k
            .create_agent("weak", "Weak", "no perms", &adapter, &ns, None, vec![])
            .unwrap();

        let err = k
            .call_tool(&run, &weak, &echo, "echo.say", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");

        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.result == AuditResult::Denied));
    }

    #[test]
    fn unknown_tool_is_an_error() {
        let (mut k, prime, _task, run, echo) = primed_kernel();
        let err = k
            .call_tool(&run, &prime, &echo, "echo.nope", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolNotFound { .. }), "got {err:?}");
    }

    #[test]
    fn agent_on_unknown_adapter_is_rejected() {
        let mut k = KernelState::new();
        let ns = k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
        let err = k
            .create_agent(
                "prime",
                "Prime",
                "x",
                &PluginId::new("missing-adapter"),
                &ns,
                None,
                vec![],
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::UnknownPlugin(_)), "got {err:?}");
    }
}
