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
    Agent, AgentId, Approval, ApprovalId, ApprovalStatus, AuditEvent, AuditResult, InstalledPlugin,
    Namespace, NamespaceId, Permission, PluginId, PluginManifest, PluginSourceKind, PrimeAction,
    PrimeContext, PrimeDisposition, PrimePlan, PrimeTurn, RiskLevel, Run, RunId, RunStatus,
    StateSummary, Task, TaskBrief, TaskId, TaskStatus,
};
use serde::{Deserialize, Serialize};

use crate::clock::Clock;
use crate::event::RunEvent;
use crate::prime::{classify_intent, decide};
use crate::KernelError;

/// The monotonic counters and logical-clock position that must be restored for a
/// resumed [`KernelState`] to keep minting ids and timestamps deterministically
/// without colliding with anything already persisted.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct KernelCounters {
    /// Logical seconds the deterministic [`Clock`] has advanced.
    pub clock_secs: u64,
    pub next_task: u64,
    pub next_run: u64,
    pub next_approval: u64,
    pub next_audit: u64,
    pub next_event: u64,
}

/// A flat, serializable export of the entire [`KernelState`].
///
/// This is the unit the [`crate::store::SqliteStore`] persists: every entity, the
/// run transcripts, the audit log, and the counters needed to resume id/timestamp
/// minting. Entities are held as ordered `Vec`s (sorted by id on export) so a
/// snapshot is byte-stable for a given logical state regardless of the live
/// `HashMap` iteration order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KernelSnapshot {
    pub plugins: Vec<PluginManifest>,
    /// Install-lifecycle records, one per installed plugin, sorted by id.
    pub installed_plugins: Vec<InstalledPlugin>,
    pub namespaces: Vec<Namespace>,
    pub agents: Vec<Agent>,
    pub tasks: Vec<Task>,
    pub runs: Vec<Run>,
    pub approvals: Vec<Approval>,
    /// Run transcripts, in emission order.
    pub run_events: Vec<RunEvent>,
    /// The append-only audit log, in emission order.
    pub audit_events: Vec<AuditEvent>,
    pub counters: KernelCounters,
}

/// The local, in-memory Relux control plane.
#[derive(Debug, Default)]
pub struct KernelState {
    plugins: HashMap<PluginId, PluginManifest>,
    installed_plugins: HashMap<PluginId, InstalledPlugin>,
    namespaces: HashMap<NamespaceId, Namespace>,
    agents: HashMap<AgentId, Agent>,
    tasks: HashMap<TaskId, Task>,
    runs: HashMap<RunId, Run>,
    approvals: HashMap<ApprovalId, Approval>,
    /// Per-run transcripts, in emission order.
    run_events: Vec<RunEvent>,
    /// The append-only audit log, in emission order.
    audit_log: Vec<AuditEvent>,

    clock: Clock,
    next_task: u64,
    next_run: u64,
    next_approval: u64,
    next_audit: u64,
    next_event: u64,
}

impl KernelState {
    pub fn new() -> Self {
        Self::default()
    }

    // --- Persistence -------------------------------------------------------

    /// Export the full control plane into a flat, serializable [`KernelSnapshot`].
    ///
    /// Entities are emitted in id order so the snapshot is stable for a given
    /// logical state; run transcripts and the audit log keep their emission
    /// order. The counters carry everything needed to resume deterministic
    /// id/timestamp minting (`docs/RELUX_MASTER_PLAN.md` section 15 Phase 1, section 17.8).
    pub fn snapshot(&self) -> KernelSnapshot {
        fn sorted<T, F>(map: &HashMap<F, T>, key: impl Fn(&T) -> &str) -> Vec<T>
        where
            T: Clone,
        {
            let mut out: Vec<T> = map.values().cloned().collect();
            out.sort_by(|a, b| key(a).cmp(key(b)));
            out
        }

        KernelSnapshot {
            plugins: sorted(&self.plugins, |p| p.id.as_str()),
            installed_plugins: sorted(&self.installed_plugins, |p| p.id.as_str()),
            namespaces: sorted(&self.namespaces, |n| n.id.as_str()),
            agents: sorted(&self.agents, |a| a.id.as_str()),
            tasks: sorted(&self.tasks, |t| t.id.as_str()),
            runs: sorted(&self.runs, |r| r.id.as_str()),
            approvals: sorted(&self.approvals, |a| a.id.as_str()),
            run_events: self.run_events.clone(),
            audit_events: self.audit_log.clone(),
            counters: KernelCounters {
                clock_secs: self.clock.secs(),
                next_task: self.next_task,
                next_run: self.next_run,
                next_approval: self.next_approval,
                next_audit: self.next_audit,
                next_event: self.next_event,
            },
        }
    }

    /// Rebuild a [`KernelState`] from a previously exported [`KernelSnapshot`].
    ///
    /// This restores state directly into the in-memory maps without re-emitting
    /// audit or run events - it is a resume, not a replay of the actions that
    /// produced the state. Counters and the logical clock are restored so newly
    /// minted ids and timestamps continue past whatever is already persisted.
    pub fn from_snapshot(snapshot: KernelSnapshot) -> Self {
        let mut state = KernelState::new();
        for plugin in snapshot.plugins {
            state.plugins.insert(plugin.id.clone(), plugin);
        }
        for installed in snapshot.installed_plugins {
            state.installed_plugins.insert(installed.id.clone(), installed);
        }
        for namespace in snapshot.namespaces {
            state.namespaces.insert(namespace.id.clone(), namespace);
        }
        for agent in snapshot.agents {
            state.agents.insert(agent.id.clone(), agent);
        }
        for task in snapshot.tasks {
            state.tasks.insert(task.id.clone(), task);
        }
        for run in snapshot.runs {
            state.runs.insert(run.id.clone(), run);
        }
        for approval in snapshot.approvals {
            state.approvals.insert(approval.id.clone(), approval);
        }
        state.run_events = snapshot.run_events;
        state.audit_log = snapshot.audit_events;
        state.clock = Clock::from_secs(snapshot.counters.clock_secs);
        state.next_task = snapshot.counters.next_task;
        state.next_run = snapshot.counters.next_run;
        state.next_approval = snapshot.counters.next_approval;
        state.next_audit = snapshot.counters.next_audit;
        state.next_event = snapshot.counters.next_event;
        state
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

    // --- Installed plugin lifecycle ----------------------------------------

    /// Register an already-validated manifest AND record its durable install
    /// metadata (`docs/RELUX_MASTER_PLAN.md` section 9.4, section 7.4).
    ///
    /// This is the kernel half of the plugin installation lifecycle that backs
    /// the future Plugins tab: the manifest enters the live index (so the plugin
    /// can be used) and an [`InstalledPlugin`] record is persisted (so the
    /// install survives restarts and is listable until removed). The install
    /// timestamp is stamped from the deterministic logical clock. Re-installing
    /// the same id cleanly replaces both records rather than duplicating them
    /// (the maps are id-keyed). The filesystem copy/extract is done by
    /// [`crate::plugin_install`]; this method does not touch disk.
    pub fn install_plugin(
        &mut self,
        manifest: PluginManifest,
        source_kind: PluginSourceKind,
        source_label: String,
        install_dir: String,
        enabled: bool,
    ) -> InstalledPlugin {
        let id = manifest.id.clone();
        let version = manifest.version.clone();
        let kind = manifest.kind.clone();
        self.register_plugin(manifest);

        let installed = InstalledPlugin {
            id: id.clone(),
            version,
            kind,
            installed_at: self.clock.tick(),
            source_kind,
            source_label,
            install_dir,
            enabled,
        };
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:install",
            Some("plugin"),
            Some(id.as_str()),
            None,
            AuditResult::Success,
            serde_json::json!({ "source": format!("{:?}", installed.source_kind) }),
        );
        self.installed_plugins.insert(id, installed.clone());
        installed
    }

    /// Remove an installed plugin's metadata and unregister its manifest
    /// (`docs/RELUX_MASTER_PLAN.md` section 7.4). The on-disk install directory is
    /// removed by [`crate::plugin_install::remove_plugin`]; this method only
    /// mutates kernel state and audits the removal. Returns the removed record.
    pub fn remove_installed_plugin(
        &mut self,
        id: &PluginId,
    ) -> Result<InstalledPlugin, KernelError> {
        let removed = self
            .installed_plugins
            .remove(id)
            .ok_or_else(|| KernelError::PluginNotInstalled(id.to_string()))?;
        self.plugins.remove(id);
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:remove",
            Some("plugin"),
            Some(id.as_str()),
            None,
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(removed)
    }

    pub fn installed_plugin(&self, id: &PluginId) -> Option<&InstalledPlugin> {
        self.installed_plugins.get(id)
    }

    /// All installed plugin records, sorted by id for deterministic listing.
    pub fn installed_plugins(&self) -> Vec<&InstalledPlugin> {
        let mut out: Vec<&InstalledPlugin> = self.installed_plugins.values().collect();
        out.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        out
    }

    pub fn installed_plugin_count(&self) -> usize {
        self.installed_plugins.len()
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

    pub fn namespace(&self, id: &NamespaceId) -> Option<&Namespace> {
        self.namespaces.get(id)
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
        if self.agents.contains_key(&agent_id) {
            return Err(KernelError::AgentExists(agent_id.to_string()));
        }
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

    /// All agents, sorted by id for deterministic listing.
    pub fn agents(&self) -> Vec<&Agent> {
        let mut out: Vec<&Agent> = self.agents.values().collect();
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        out
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

    /// All tasks, sorted by id for deterministic listing.
    pub fn tasks(&self) -> Vec<&Task> {
        let mut out: Vec<&Task> = self.tasks.values().collect();
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        out
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

    // --- Approvals ---------------------------------------------------------

    /// Raise a human approval request for a risky proposed action
    /// (`docs/RELUX_MASTER_PLAN.md` section 9.9, section 10.3). The request is recorded as
    /// `Pending` and audited; nothing about the proposed action is performed -
    /// this is the seam that stops Prime (or an agent) acting silently.
    pub fn request_approval(
        &mut self,
        requested_by: &str,
        action: &str,
        reason: &str,
        risk: RiskLevel,
        namespace: Option<&NamespaceId>,
    ) -> ApprovalId {
        self.next_approval += 1;
        let id = ApprovalId::new(format!("appr_{:04}", self.next_approval));
        let created = self.clock.tick();
        let approval = Approval {
            id: id.clone(),
            requested_by: requested_by.to_string(),
            action: action.to_string(),
            reason: reason.to_string(),
            risk: risk.clone(),
            status: ApprovalStatus::Pending,
            approved_by: None,
            namespace_id: namespace.cloned(),
            created_at: created,
            resolved_at: None,
        };
        self.record_audit(
            "agent",
            requested_by,
            "approval:request",
            Some("approval"),
            Some(id.as_str()),
            namespace,
            AuditResult::Success,
            serde_json::json!({ "risk": format!("{:?}", risk), "action": action }),
        );
        self.approvals.insert(id.clone(), approval);
        id
    }

    /// Record a human's decision on a pending approval
    /// (`docs/RELUX_MASTER_PLAN.md` section 9.9). This records and audits the
    /// decision only; re-running the originally proposed action on approval is
    /// deliberately out of scope for this Prime Core slice (a later slice wires
    /// deferred-action execution).
    pub fn resolve_approval(
        &mut self,
        id: &ApprovalId,
        approve: bool,
        approver: &str,
    ) -> Result<(), KernelError> {
        let resolved = self.clock.tick();
        let namespace = {
            let approval = self
                .approvals
                .get_mut(id)
                .ok_or_else(|| KernelError::UnknownApproval(id.to_string()))?;
            approval.status = if approve {
                ApprovalStatus::Approved
            } else {
                ApprovalStatus::Rejected
            };
            approval.approved_by = Some(approver.to_string());
            approval.resolved_at = Some(resolved);
            approval.namespace_id.clone()
        };
        self.record_audit(
            "user",
            approver,
            "approval:resolve",
            Some("approval"),
            Some(id.as_str()),
            namespace.as_ref(),
            if approve {
                AuditResult::Approved
            } else {
                AuditResult::Rejected
            },
            serde_json::Value::Null,
        );
        Ok(())
    }

    pub fn approval(&self, id: &ApprovalId) -> Option<&Approval> {
        self.approvals.get(id)
    }

    pub fn approval_count(&self) -> usize {
        self.approvals.len()
    }

    pub fn pending_approval_count(&self) -> usize {
        self.approvals
            .values()
            .filter(|a| matches!(a.status, ApprovalStatus::Pending))
            .count()
    }

    // --- Prime -------------------------------------------------------------

    /// Project the current control plane into the grounded [`StateSummary`] that
    /// Prime reasons over (`docs/RELUX_MASTER_PLAN.md` section 10.1, section 17.1). This is
    /// the in-memory stand-in for the context a real LLM Prime would receive.
    pub fn inspect_state(&self) -> StateSummary {
        let runs_active = self
            .runs
            .values()
            .filter(|r| matches!(r.status, RunStatus::Running))
            .count();

        let mut tasks_open = 0usize;
        let mut waiting = 0usize;
        let mut blocked = 0usize;
        let mut failed = 0usize;
        for t in self.tasks.values() {
            if !matches!(
                t.status,
                TaskStatus::Completed
                    | TaskStatus::Failed
                    | TaskStatus::Cancelled
                    | TaskStatus::Expired
            ) {
                tasks_open += 1;
            }
            match t.status {
                TaskStatus::WaitingForApproval => waiting += 1,
                TaskStatus::Blocked => blocked += 1,
                TaskStatus::Failed => failed += 1,
                _ => {}
            }
        }

        // Sort by id so `queued` and `recent` are deterministic regardless of the
        // HashMap's iteration order; ids are zero-padded so lexical == numeric.
        let mut sorted: Vec<&Task> = self.tasks.values().collect();
        sorted.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        let queued: Vec<TaskBrief> = sorted
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Queued))
            .map(|t| task_brief(t))
            .collect();
        let recent: Vec<TaskBrief> = sorted.iter().rev().take(5).map(|t| task_brief(t)).collect();

        StateSummary {
            plugins: self.plugins.len(),
            agents: self.agents.len(),
            tasks_total: self.tasks.len(),
            tasks_open,
            runs_active,
            tasks_waiting_approval: waiting,
            tasks_blocked: blocked,
            tasks_failed: failed,
            pending_approvals: self.pending_approval_count(),
            queued,
            recent,
        }
    }

    /// Handle one user message as Prime (`docs/RELUX_MASTER_PLAN.md` section 10, section 16).
    ///
    /// The flow is: inspect state -> classify intent -> decide a grounded plan ->
    /// execute it through the kernel. Safe, in-scope actions (create task, start
    /// the single ready run) run directly; risky actions become an approval
    /// request and are never performed here; everything else is a grounded reply
    /// or a clarifying question. Prime routes only through existing kernel
    /// actions - it never mutates state behind their back (section 7.1, section 10.2).
    pub fn prime_turn(
        &mut self,
        ctx: &PrimeContext,
        message: &str,
    ) -> Result<PrimeTurn, KernelError> {
        let summary = self.inspect_state();
        let intent = classify_intent(message);
        let plan = decide(message, &intent, &summary);

        self.record_audit(
            "agent",
            ctx.agent.as_str(),
            "prime:turn",
            Some("message"),
            None,
            Some(&ctx.namespace),
            AuditResult::Success,
            serde_json::json!({ "intent": format!("{:?}", intent) }),
        );

        match plan {
            PrimePlan::Reply { text } => Ok(PrimeTurn {
                intent,
                reply: text,
                disposition: PrimeDisposition::Answered,
                action: None,
                created_task: None,
                started_run: None,
                approval: None,
            }),
            PrimePlan::Clarify { text } => Ok(PrimeTurn {
                intent,
                reply: text,
                disposition: PrimeDisposition::NeedsClarification,
                action: None,
                created_task: None,
                started_run: None,
                approval: None,
            }),
            PrimePlan::Act { action, text } => self.prime_execute(ctx, intent, action, text),
            PrimePlan::Propose {
                action,
                reason,
                risk,
                text,
            } => {
                let rendered = describe_action(&action);
                let approval = self.request_approval(
                    ctx.agent.as_str(),
                    &rendered,
                    &reason,
                    risk.clone(),
                    Some(&ctx.namespace),
                );
                let reply = format!(
                    "{text} I will not do this without approval. I have logged {approval} ({risk:?} risk): {reason}"
                );
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::AwaitingApproval,
                    action: Some(action),
                    created_task: None,
                    started_run: None,
                    approval: Some(approval),
                })
            }
        }
    }

    /// Execute the safe `Act` actions Prime is allowed to perform directly.
    ///
    /// `decide` only ever emits `CreateTask` and `StartRun` as `Act`s; any other
    /// action arriving here is treated as not-yet-wired and surfaced honestly
    /// rather than silently dropped.
    fn prime_execute(
        &mut self,
        ctx: &PrimeContext,
        intent: relux_core::PrimeIntent,
        action: PrimeAction,
        text: String,
    ) -> Result<PrimeTurn, KernelError> {
        match &action {
            PrimeAction::CreateTask { title } => {
                let task = self.create_task(
                    title,
                    serde_json::json!({ "prime_request": title }),
                    &ctx.actor,
                    &ctx.namespace,
                    vec![],
                );
                // Assign to Prime so the work is immediately runnable when the
                // user says "start it"; assigning to self is within Prime's scope.
                self.assign_task(&task, &ctx.agent)?;
                let reply = format!(
                    "{text} Created {task} and assigned it to {}. Say \"start it\" when you want me to run it.",
                    ctx.agent
                );
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: Some(task),
                    started_run: None,
                    approval: None,
                })
            }
            PrimeAction::CreateAndRunTask { title } => {
                let task = self.create_task(
                    title,
                    serde_json::json!({ "prime_request": title }),
                    &ctx.actor,
                    &ctx.namespace,
                    vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
                );
                self.assign_task(&task, &ctx.agent)?;
                let run = self.start_run(&task)?;

                // Perform the echo cycle on the task's own input, so the loop
                // proves the real payload round-trips rather than a fixed string.
                let echo_plugin = PluginId::new("relux-tools-echo");
                let input = self
                    .task(&task)
                    .map(|t| t.input.clone())
                    .unwrap_or_else(|| serde_json::json!({ "prime_request": title }));
                self.call_tool(&run, &ctx.agent, &echo_plugin, "echo.say", input)?;
                self.complete_run(&run, "echo.say returned the input unchanged")?;
                self.complete_task(&task)?;

                let reply =
                    format!("{text} Created {task}, started {run}, and completed the echo cycle.");
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: Some(task),
                    started_run: Some(run),
                    approval: None,
                })
            }
            PrimeAction::StartRun { task_id } => {
                let tid = TaskId::new(task_id.clone());
                let run = self.start_run(&tid)?;

                // Plain start it on one queued task also completes run and task through echo.
                let echo_plugin = PluginId::new("relux-tools-echo");
                let input = self
                    .task(&tid)
                    .map(|t| t.input.clone())
                    .unwrap_or_else(|| serde_json::json!({}));
                self.call_tool(&run, &ctx.agent, &echo_plugin, "echo.say", input)?;
                self.complete_run(&run, "echo.say returned the input unchanged")?;
                self.complete_task(&tid)?;

                let reply = format!("{text} Started {run} and completed the echo cycle.");
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: None,
                    started_run: Some(run),
                    approval: None,
                })
            }
            other => Ok(PrimeTurn {
                intent,
                reply: format!(
                    "I planned an action I cannot execute yet: {}.",
                    describe_action(other)
                ),
                disposition: PrimeDisposition::NeedsClarification,
                action: Some(action.clone()),
                created_task: None,
                started_run: None,
                approval: None,
            }),
        }
    }

    // --- Inspection --------------------------------------------------------

    pub fn run(&self, id: &RunId) -> Option<&Run> {
        self.runs.get(id)
    }

    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// All runs, sorted by id for deterministic listing.
    pub fn runs(&self) -> Vec<&Run> {
        let mut out: Vec<&Run> = self.runs.values().collect();
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        out
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

/// Project a `Task` into the compact `TaskBrief` Prime speaks about.
fn task_brief(t: &Task) -> TaskBrief {
    TaskBrief {
        id: t.id.clone(),
        title: t.title.clone(),
        status: t.status.clone(),
        assigned_agent: t.assigned_agent.clone(),
    }
}

/// Render a `PrimeAction` as a one-line human-readable string for approvals and
/// audit metadata.
fn describe_action(action: &PrimeAction) -> String {
    match action {
        PrimeAction::CreateTask { title } => format!("create task \"{title}\""),
        PrimeAction::StartRun { task_id } => format!("start a run for {task_id}"),
        PrimeAction::GrantPermission {
            subject_id,
            permission,
        } => format!("grant {permission} to {subject_id}"),
        PrimeAction::InstallPlugin { plugin_id } => format!("install plugin {plugin_id}"),
        PrimeAction::ConfigurePlugin { plugin_id } => format!("configure plugin {plugin_id}"),
        PrimeAction::CreateAgent {
            name,
            adapter_plugin,
        } => format!("create agent {name} on adapter {adapter_plugin}"),
        other => format!("{other:?}"),
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
            .any(|e| e.action == "tool:relux-tools-echo:say" && e.result == AuditResult::Success));
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
        assert!(
            matches!(err, KernelError::PermissionDenied { .. }),
            "got {err:?}"
        );

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
        assert!(
            matches!(err, KernelError::ToolNotFound { .. }),
            "got {err:?}"
        );
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

    /// A kernel with plugins, a workspace namespace, and a Prime agent - the
    /// minimum for driving Prime chat - plus the matching `PrimeContext`.
    fn prime_chat_kernel() -> (KernelState, PrimeContext) {
        let mut k = KernelState::new();
        k.register_plugin(echo_manifest());
        k.register_plugin(adapter_manifest());
        let ns = k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
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
        let ctx = PrimeContext {
            namespace: ns,
            agent: prime,
            actor: "founder".to_string(),
        };
        (k, ctx)
    }

    #[test]
    fn greeting_is_answered_without_changing_state() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "hey").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::Greeting);
        assert_eq!(turn.disposition, PrimeDisposition::Answered);
        assert!(turn.created_task.is_none());
        assert_eq!(k.task_count(), 0, "a greeting must not create work");
    }

    #[test]
    fn task_creation_then_run_start_walks_the_loop() {
        let (mut k, ctx) = prime_chat_kernel();

        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        assert_eq!(created.disposition, PrimeDisposition::Executed);
        let task_id = created.created_task.expect("a task was created");
        // Prime assigned it to itself, so it is Queued and runnable.
        assert_eq!(k.task(&task_id).unwrap().status, TaskStatus::Queued);
        assert_eq!(
            k.task(&task_id).unwrap().assigned_agent.as_ref(),
            Some(&ctx.agent)
        );

        let started = k.prime_turn(&ctx, "start it").unwrap();
        assert_eq!(started.disposition, PrimeDisposition::Executed);
        let run_id = started.started_run.expect("a run was started");
        // start it now completes the task.
        assert_eq!(k.run(&run_id).unwrap().status, RunStatus::Completed);
        assert_eq!(k.task(&task_id).unwrap().status, TaskStatus::Completed);
    }

    #[test]
    fn create_and_run_task_completes_in_one_turn() {
        let (mut k, ctx) = prime_chat_kernel();

        let turn = k
            .prime_turn(&ctx, "create a task to echo hello and run it")
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        let task_id = turn.created_task.expect("a task was created");
        let run_id = turn.started_run.expect("a run was started");

        assert_eq!(k.task(&task_id).unwrap().status, TaskStatus::Completed);
        assert_eq!(k.run(&run_id).unwrap().status, RunStatus::Completed);

        // Transcript shows full cycle
        let kinds: Vec<&str> = k
            .run_events(&run_id)
            .iter()
            .map(|e| e.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["run_started", "tool_call", "run_completed"]);
    }

    #[test]
    fn risky_permission_change_is_gated_behind_approval() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "give this agent GitHub access").unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        let approval = turn.approval.expect("an approval was raised");
        assert_eq!(
            k.approval(&approval).unwrap().status,
            ApprovalStatus::Pending
        );
        assert_eq!(k.pending_approval_count(), 1);
        // Nothing was granted; the request is only logged.
        assert!(k.audit_log().iter().any(|e| e.action == "approval:request"));
    }

    #[test]
    fn resolving_an_approval_records_the_decision() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "install relux-tools-github").unwrap();
        let approval = turn.approval.expect("an approval was raised");

        k.resolve_approval(&approval, true, "founder").unwrap();
        assert_eq!(
            k.approval(&approval).unwrap().status,
            ApprovalStatus::Approved
        );
        assert_eq!(k.pending_approval_count(), 0);
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "approval:resolve" && e.result == AuditResult::Approved));
    }

    #[test]
    fn status_question_is_grounded_in_state() {
        let (mut k, ctx) = prime_chat_kernel();
        // No work yet -> Prime reports idle, does not invent runs.
        let idle = k.prime_turn(&ctx, "what is going on?").unwrap();
        assert_eq!(idle.intent, relux_core::PrimeIntent::StatusQuestion);
        assert!(idle.reply.contains("idle"), "got: {}", idle.reply);

        // After creating a task, the status reflects open work.
        k.prime_turn(&ctx, "fix the flaky test").unwrap();
        let busy = k.prime_turn(&ctx, "what is going on?").unwrap();
        assert!(busy.reply.contains("open task"), "got: {}", busy.reply);
    }

    #[test]
    fn snapshot_round_trip_preserves_state_and_counters() {
        let (mut k, ctx) = prime_chat_kernel();
        // Walk a full loop so every map, the transcript, and the audit log are
        // non-empty and the counters have advanced.
        let turn = k
            .prime_turn(&ctx, "create a task to echo hello and run it")
            .unwrap();
        let task = turn.created_task.expect("a task was created");
        let run = turn.started_run.expect("a run was started");

        let before = k.snapshot();
        let restored = KernelState::from_snapshot(before.clone());

        // Entity counts and key states survive the round trip.
        assert_eq!(restored.plugin_count(), k.plugin_count());
        assert_eq!(restored.namespace_count(), k.namespace_count());
        assert_eq!(restored.agent_count(), k.agent_count());
        assert_eq!(restored.task_count(), k.task_count());
        assert_eq!(restored.run_count(), k.run_count());
        assert_eq!(
            restored.task(&task).unwrap().status,
            TaskStatus::Completed
        );
        assert_eq!(restored.run(&run).unwrap().status, RunStatus::Completed);
        assert_eq!(
            restored.run_events(&run).len(),
            k.run_events(&run).len()
        );
        assert_eq!(restored.audit_log().len(), k.audit_log().len());

        // Counters resume: the next snapshot from the restored kernel is identical,
        // and a fresh action mints the *next* id rather than colliding.
        let after = restored.snapshot();
        assert_eq!(after.counters.next_task, before.counters.next_task);
        assert_eq!(after.counters.next_run, before.counters.next_run);
        assert_eq!(after.counters.clock_secs, before.counters.clock_secs);

        let mut resumed = restored;
        let next = resumed
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap()
            .created_task
            .expect("a second task was created");
        assert_ne!(next, task, "resumed kernel must not reuse a task id");
    }

    #[test]
    fn listing_tasks_and_runs_is_sorted_and_complete() {
        let (mut k, prime, task, run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");

        // Add another task and run.
        let t2 = k.create_task("t2", serde_json::json!({}), "founder", &ns, vec![]);
        k.assign_task(&t2, &prime).unwrap();
        let r2 = k.start_run(&t2).unwrap();

        let tasks = k.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, task);
        assert_eq!(tasks[1].id, t2);

        let runs = k.runs();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, run);
        assert_eq!(runs[1].id, r2);
    }

    #[test]
    fn listing_agents_is_sorted_and_complete_and_can_be_created() {
        let (mut k, prime, task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");

        // Create another agent
        let agent2_id = k
            .create_agent(
                "agent-two",
                "Agent Two",
                "Second agent.",
                &adapter,
                &ns,
                None,
                vec![],
            )
            .unwrap();

        let agents = k.agents();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, agent2_id);
        assert_eq!(agents[1].id, prime);

        // Try to create an agent with a colliding ID
        let err = k
            .create_agent(
                "agent-two",
                "Agent Two Collision",
                "Should fail.",
                &adapter,
                &ns,
                None,
                vec![],
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::AgentExists(_)));

        // Assign task to agent2
        k.assign_task(&task, &agent2_id).unwrap();
        assert_eq!(
            k.task(&task).unwrap().assigned_agent.as_ref(),
            Some(&agent2_id)
        );
    }
}
