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

use relux_core::adapter::{
    clamp_adapter_max_output, clamp_adapter_timeout, recognize_adapter_kind, AdapterKind,
    AdapterRuntimeConfig, AdapterRuntimeState, AdapterRuntimeStatus,
};
use relux_core::agent::AgentStatus;
use relux_core::namespace::NamespaceKind;
use relux_core::plugin::PluginKind;
use relux_core::{
    clamp_runtime_timeout, plan_orchestration, validate_loopback_url, Agent, AgentId, Approval,
    ApprovalId, ApprovalStatus, AuditEvent, AuditResult, InstalledPlugin, Namespace, NamespaceId,
    Orchestration, OrchestrationBatchResult, OrchestrationId, OrchestrationStatus,
    OrchestrationStep, Permission, PluginId, PluginManifest, PluginSourceKind, PrimeAction,
    PrimeAutonomyConfig, PrimeAutonomyTickResult, PrimeContext, PrimeDisposition, PrimePlan,
    PrimeTurn, RiskLevel, RuntimeKind, Run, RunId, RunStatus, StateSummary, StepOutcome, Task,
    TaskBrief, TaskId, TaskStatus, ToolDescriptor, ToolExecutability, ToolInvocationResult,
    ToolRuntimeConfig,
};
use serde::{Deserialize, Serialize};

use crate::clock::Clock;
use crate::event::RunEvent;
use crate::prime::{brainstorm_task_candidate, classify_intent, decide, plan_goal};
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
    /// Next orchestration id. Defaulted so snapshots from before multi-agent
    /// autonomy load cleanly (orchestrations start at 0).
    #[serde(default)]
    pub next_orchestration: u64,
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
    /// Per-plugin tool runtime configs (HTTP loopback), sorted by plugin id.
    #[serde(default)]
    pub tool_runtime_configs: Vec<ToolRuntimeConfig>,
    /// Per-adapter CLI runtime configs (local coding-agent CLIs), sorted by
    /// plugin id. Defaulted so older snapshots load cleanly.
    #[serde(default)]
    pub adapter_runtime_configs: Vec<AdapterRuntimeConfig>,
    pub namespaces: Vec<Namespace>,
    pub agents: Vec<Agent>,
    pub tasks: Vec<Task>,
    pub runs: Vec<Run>,
    pub approvals: Vec<Approval>,
    /// Run transcripts, in emission order.
    pub run_events: Vec<RunEvent>,
    /// The append-only audit log, in emission order.
    pub audit_events: Vec<AuditEvent>,
    pub prime_autonomy_config: PrimeAutonomyConfig,
    /// Durable Prime orchestrations (goal -> briefs -> agents -> runs), sorted by
    /// id. Defaulted so older snapshots load cleanly.
    #[serde(default)]
    pub orchestrations: Vec<Orchestration>,
    pub counters: KernelCounters,
}

/// The local, in-memory Relux control plane.
#[derive(Debug, Default)]
pub struct KernelState {
    plugins: HashMap<PluginId, PluginManifest>,
    installed_plugins: HashMap<PluginId, InstalledPlugin>,
    /// Per-plugin tool runtime configs (HTTP loopback), keyed by plugin id.
    tool_runtime_configs: HashMap<PluginId, ToolRuntimeConfig>,
    /// Per-adapter CLI runtime configs, keyed by plugin id.
    adapter_runtime_configs: HashMap<PluginId, AdapterRuntimeConfig>,
    namespaces: HashMap<NamespaceId, Namespace>,
    agents: HashMap<AgentId, Agent>,
    tasks: HashMap<TaskId, Task>,
    runs: HashMap<RunId, Run>,
    pub approvals: HashMap<ApprovalId, Approval>,
    /// Durable Prime orchestrations, keyed by id.
    orchestrations: HashMap<OrchestrationId, Orchestration>,
    /// Per-run transcripts, in emission order.
    run_events: Vec<RunEvent>,
    /// The append-only audit log, in emission order.
    audit_log: Vec<AuditEvent>,
    pub prime_autonomy_config: PrimeAutonomyConfig,
    clock: Clock,
    next_task: u64,
    next_run: u64,
    next_approval: u64,
    next_audit: u64,
    next_event: u64,
    next_orchestration: u64,
}

/// The outcome of idempotently refreshing one bundled plugin manifest into the
/// live control plane (`docs/RELUX_MASTER_PLAN.md` section 9.4, section 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundledRefresh {
    /// The bundled plugin was not present and was installed.
    Added,
    /// The bundled plugin was already installed; its manifest/metadata changed
    /// and the record was updated in place (no duplicate).
    Updated,
    /// The bundled plugin was already installed and is byte-identical; nothing
    /// changed, so no audit noise was emitted.
    Unchanged,
    /// A plugin with this id is installed from a non-bundled source (a user
    /// install); it was left untouched rather than being overwritten.
    SkippedUserInstalled,
}

/// A tally of what an idempotent bundled-plugin refresh did across all shipped
/// manifests. Used by the CLI/boot path to report (and persist) changes.
#[derive(Debug, Clone, Default)]
pub struct BundledRefreshSummary {
    /// Ids of newly installed bundled plugins.
    pub added: Vec<String>,
    /// Ids of bundled plugins whose record was updated in place.
    pub updated: Vec<String>,
    /// Count of bundled plugins that were already up to date.
    pub unchanged: usize,
    /// Ids skipped because a same-id user-installed plugin exists.
    pub skipped_user_installed: Vec<String>,
}

impl BundledRefreshSummary {
    /// True if the refresh added or updated any bundled record (i.e. the store
    /// must be saved to persist the change).
    pub fn changed(&self) -> bool {
        !self.added.is_empty() || !self.updated.is_empty()
    }
}

/// True if two manifests are logically identical (compared by their canonical
/// JSON form, since [`PluginManifest`] does not derive `PartialEq`).
fn manifests_equal(a: &PluginManifest, b: &PluginManifest) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(va), Ok(vb)) => va == vb,
        // If either fails to serialize (it never should), treat as different so
        // the refresh re-installs rather than silently skipping an update.
        _ => false,
    }
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
            tool_runtime_configs: sorted(&self.tool_runtime_configs, |c| c.plugin_id.as_str()),
            adapter_runtime_configs: sorted(&self.adapter_runtime_configs, |c| {
                c.plugin_id.as_str()
            }),
            namespaces: sorted(&self.namespaces, |n| n.id.as_str()),
            agents: sorted(&self.agents, |a| a.id.as_str()),
            tasks: sorted(&self.tasks, |t| t.id.as_str()),
            runs: sorted(&self.runs, |r| r.id.as_str()),
            approvals: sorted(&self.approvals, |a| a.id.as_str()),
            run_events: self.run_events.clone(),
            audit_events: self.audit_log.clone(),
            prime_autonomy_config: self.prime_autonomy_config.clone(),
            orchestrations: sorted(&self.orchestrations, |o| o.id.as_str()),
            counters: KernelCounters {
                clock_secs: self.clock.secs(),
                next_task: self.next_task,
                next_run: self.next_run,
                next_approval: self.next_approval,
                next_audit: self.next_audit,
                next_event: self.next_event,
                next_orchestration: self.next_orchestration,
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
            state
                .installed_plugins
                .insert(installed.id.clone(), installed);
        }
        for cfg in snapshot.tool_runtime_configs {
            state
                .tool_runtime_configs
                .insert(PluginId::new(cfg.plugin_id.clone()), cfg);
        }
        for cfg in snapshot.adapter_runtime_configs {
            state
                .adapter_runtime_configs
                .insert(PluginId::new(cfg.plugin_id.clone()), cfg);
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
        for orchestration in snapshot.orchestrations {
            state
                .orchestrations
                .insert(orchestration.id.clone(), orchestration);
        }
        state.run_events = snapshot.run_events;
        state.audit_log = snapshot.audit_events;
        state.clock = Clock::from_secs(snapshot.counters.clock_secs);
        state.next_task = snapshot.counters.next_task;
        state.next_run = snapshot.counters.next_run;
        state.next_approval = snapshot.counters.next_approval;
        state.next_audit = snapshot.counters.next_audit;
        state.next_event = snapshot.counters.next_event;
        state.next_orchestration = snapshot.counters.next_orchestration;
        state.prime_autonomy_config = snapshot.prime_autonomy_config;
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
        // Drop any runtime config so a re-install of the same id does not inherit
        // a stale loopback endpoint or CLI adapter runtime.
        self.tool_runtime_configs.remove(id);
        self.adapter_runtime_configs.remove(id);
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

    /// Idempotently refresh ONE shipped bundled plugin manifest into the live
    /// control plane (`docs/RELUX_MASTER_PLAN.md` section 9.4, section 7.4).
    ///
    /// This is the per-manifest core behind [`crate::refresh_bundled_plugins`].
    /// It is safe to call on every load, for an existing store as much as a fresh
    /// one:
    ///
    /// - If the id is not installed, it is installed as a protected
    ///   [`PluginSourceKind::Bundled`] record (enabled) - this is how an older DB
    ///   picks up newly shipped capabilities without a reset.
    /// - If it is already installed as `Bundled`, the record is updated in place
    ///   ONLY when the manifest or its install metadata changed, preserving the
    ///   operator's `enabled` choice and never duplicating records. An unchanged
    ///   manifest is a no-op (no audit noise).
    /// - If a plugin with the same id is installed from a NON-bundled source (a
    ///   user install), it is left untouched - the refresh never overwrites a
    ///   user-installed plugin.
    ///
    /// Per-plugin runtime config (HTTP loopback / CLI adapter) and all other
    /// local state are untouched: this only ever re-registers the manifest and
    /// re-stamps the install record.
    pub fn refresh_bundled_plugin(
        &mut self,
        manifest: PluginManifest,
        source_label: String,
        install_dir: String,
    ) -> BundledRefresh {
        let id = manifest.id.clone();
        let existing = match self.installed_plugins.get(&id) {
            Some(existing) if existing.source_kind != PluginSourceKind::Bundled => {
                return BundledRefresh::SkippedUserInstalled;
            }
            Some(existing) => Some((
                existing.enabled,
                existing.version == manifest.version
                    && existing.install_dir == install_dir
                    && existing.source_label == source_label,
            )),
            None => None,
        };

        match existing {
            None => {
                self.install_plugin(
                    manifest,
                    PluginSourceKind::Bundled,
                    source_label,
                    install_dir,
                    true,
                );
                BundledRefresh::Added
            }
            Some((enabled, meta_same)) => {
                let manifest_same = self
                    .plugins
                    .get(&id)
                    .map(|stored| manifests_equal(stored, &manifest))
                    .unwrap_or(false);
                if meta_same && manifest_same {
                    BundledRefresh::Unchanged
                } else {
                    // Preserve the operator's enabled choice; re-stamp the rest.
                    self.install_plugin(
                        manifest,
                        PluginSourceKind::Bundled,
                        source_label,
                        install_dir,
                        enabled,
                    );
                    BundledRefresh::Updated
                }
            }
        }
    }

    // --- Tool runtime config (HTTP loopback) -------------------------------

    /// Configure (or update) the HTTP loopback runtime for an installed plugin
    /// (`docs/RELUX_MASTER_PLAN.md` section 8.2, section 18). Validates the
    /// loopback URL, clamps the timeout, and persists the config as enabled by
    /// default. The plugin must be installed and must NOT be a bundled fixture -
    /// the built-in echo/status tools already run deterministically and a runtime
    /// config could only confuse that.
    ///
    /// This never stores secrets: only the base URL, the enabled flag, and the
    /// timeout. The actual loopback server is started separately by the operator.
    pub fn configure_tool_runtime(
        &mut self,
        plugin_id: &PluginId,
        base_url: &str,
        enabled: bool,
        timeout_ms: Option<u64>,
    ) -> Result<ToolRuntimeConfig, KernelError> {
        let installed = self
            .installed_plugins
            .get(plugin_id)
            .ok_or_else(|| KernelError::PluginNotInstalled(plugin_id.to_string()))?;
        if installed.source_kind == PluginSourceKind::Bundled {
            return Err(KernelError::InvalidRuntimeConfig {
                plugin: plugin_id.to_string(),
                message: "bundled plugins already run as built-in deterministic tools; \
                          configuring a loopback runtime for them is not allowed"
                    .to_string(),
            });
        }
        let base_url = base_url.trim().to_string();
        validate_loopback_url(&base_url).map_err(|e| KernelError::InvalidRuntimeConfig {
            plugin: plugin_id.to_string(),
            message: e.to_string(),
        })?;

        let config = ToolRuntimeConfig {
            plugin_id: plugin_id.as_str().to_string(),
            kind: RuntimeKind::HttpLoopback,
            base_url,
            enabled,
            timeout_ms: clamp_runtime_timeout(timeout_ms),
        };
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:runtime_configure",
            Some("plugin"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::json!({
                "kind": config.kind.as_str(),
                "enabled": config.enabled,
                "timeout_ms": config.timeout_ms,
            }),
        );
        self.tool_runtime_configs
            .insert(plugin_id.clone(), config.clone());
        Ok(config)
    }

    /// Disable the runtime for a plugin, keeping its base URL so it can be
    /// re-enabled. Errors if no runtime is configured.
    pub fn disable_tool_runtime(
        &mut self,
        plugin_id: &PluginId,
    ) -> Result<ToolRuntimeConfig, KernelError> {
        let config = self
            .tool_runtime_configs
            .get_mut(plugin_id)
            .ok_or_else(|| KernelError::RuntimeNotConfigured {
                plugin: plugin_id.to_string(),
            })?;
        config.enabled = false;
        let config = config.clone();
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:runtime_disable",
            Some("plugin"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(config)
    }

    /// Remove a plugin's runtime config entirely. Errors if none is configured.
    pub fn remove_tool_runtime(&mut self, plugin_id: &PluginId) -> Result<(), KernelError> {
        self.tool_runtime_configs
            .remove(plugin_id)
            .ok_or_else(|| KernelError::RuntimeNotConfigured {
                plugin: plugin_id.to_string(),
            })?;
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:runtime_remove",
            Some("plugin"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(())
    }

    pub fn tool_runtime_config(&self, plugin_id: &PluginId) -> Option<&ToolRuntimeConfig> {
        self.tool_runtime_configs.get(plugin_id)
    }

    /// All runtime configs, sorted by plugin id for deterministic listing.
    pub fn tool_runtime_configs(&self) -> Vec<&ToolRuntimeConfig> {
        let mut out: Vec<&ToolRuntimeConfig> = self.tool_runtime_configs.values().collect();
        out.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
        out
    }

    // --- Adapter runtime config (local CLI adapters) -----------------------

    /// Configure (or update) the local CLI runtime for an installed Adapter
    /// plugin (`docs/RELUX_MASTER_PLAN.md` section 8.1, Adapter Runtime v1).
    ///
    /// The plugin must be installed and of kind `Adapter`. The local-prime
    /// deterministic adapter is refused (it has no external binary). For an
    /// unrecognized adapter the kind is a generic [`AdapterKind::Command`], which
    /// requires an explicit `command`. Partial updates merge with any existing
    /// config; the result is persisted and audited. No secrets are stored.
    ///
    /// CLI adapters are **disabled by default**: a brand-new config defaults to
    /// `enabled = false` unless the caller explicitly enables it.
    #[allow(clippy::too_many_arguments)]
    pub fn configure_adapter_runtime(
        &mut self,
        plugin_id: &PluginId,
        enabled: Option<bool>,
        command: Option<String>,
        timeout_seconds: Option<u64>,
        max_output_bytes: Option<u64>,
        working_dir: Option<String>,
    ) -> Result<AdapterRuntimeConfig, KernelError> {
        let manifest = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| KernelError::PluginNotInstalled(plugin_id.to_string()))?;
        if manifest.kind != PluginKind::Adapter {
            return Err(KernelError::NotAnAdapter {
                plugin: plugin_id.to_string(),
            });
        }

        // Resolve the runtime kind from the well-known id, defaulting to a generic
        // command adapter for an unrecognized adapter plugin.
        let kind = match recognize_adapter_kind(plugin_id.as_str()) {
            Some(AdapterKind::LocalPrime) => {
                return Err(KernelError::AdapterNotConfigurable {
                    plugin: plugin_id.to_string(),
                    message: "the local Prime adapter runs the deterministic echo path and \
                              has no external CLI to configure"
                        .to_string(),
                });
            }
            Some(k) => k,
            None => AdapterKind::Command,
        };

        let existing = self.adapter_runtime_configs.get(plugin_id).cloned();
        let command = command
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .or_else(|| existing.as_ref().and_then(|c| c.command.clone()));
        let working_dir = working_dir
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .or_else(|| existing.as_ref().and_then(|c| c.working_dir.clone()));
        let timeout_seconds =
            clamp_adapter_timeout(timeout_seconds.or(existing.as_ref().map(|c| c.timeout_seconds)));
        let max_output_bytes = clamp_adapter_max_output(
            max_output_bytes.or(existing.as_ref().map(|c| c.max_output_bytes)),
        );
        // Disabled by default on first configure; preserve prior state otherwise.
        let enabled = enabled
            .or_else(|| existing.as_ref().map(|c| c.enabled))
            .unwrap_or(false);

        let config = AdapterRuntimeConfig {
            plugin_id: plugin_id.as_str().to_string(),
            kind: kind.clone(),
            enabled,
            command,
            timeout_seconds,
            max_output_bytes,
            working_dir,
        };

        // A generic command adapter must resolve to a launchable binary.
        if config.resolved_command().is_none() {
            return Err(KernelError::InvalidAdapterConfig {
                plugin: plugin_id.to_string(),
                message: "a generic command adapter requires an explicit command".to_string(),
            });
        }

        self.record_audit(
            "kernel",
            "kernel",
            "adapter:runtime_configure",
            Some("adapter"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::json!({
                "kind": config.kind.as_str(),
                "enabled": config.enabled,
                "timeout_seconds": config.timeout_seconds,
                "max_output_bytes": config.max_output_bytes,
            }),
        );
        self.adapter_runtime_configs
            .insert(plugin_id.clone(), config.clone());
        Ok(config)
    }

    /// Disable an adapter's CLI runtime, keeping its config so it can be
    /// re-enabled. Errors if no runtime is configured.
    pub fn disable_adapter_runtime(
        &mut self,
        plugin_id: &PluginId,
    ) -> Result<AdapterRuntimeConfig, KernelError> {
        let config = self.adapter_runtime_configs.get_mut(plugin_id).ok_or_else(|| {
            KernelError::AdapterRuntimeNotConfigured {
                plugin: plugin_id.to_string(),
            }
        })?;
        config.enabled = false;
        let config = config.clone();
        self.record_audit(
            "kernel",
            "kernel",
            "adapter:runtime_disable",
            Some("adapter"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(config)
    }

    /// Remove an adapter's runtime config entirely. Errors if none is configured.
    pub fn remove_adapter_runtime(&mut self, plugin_id: &PluginId) -> Result<(), KernelError> {
        self.adapter_runtime_configs.remove(plugin_id).ok_or_else(|| {
            KernelError::AdapterRuntimeNotConfigured {
                plugin: plugin_id.to_string(),
            }
        })?;
        self.record_audit(
            "kernel",
            "kernel",
            "adapter:runtime_remove",
            Some("adapter"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(())
    }

    pub fn adapter_runtime_config(&self, plugin_id: &PluginId) -> Option<&AdapterRuntimeConfig> {
        self.adapter_runtime_configs.get(plugin_id)
    }

    /// The honest status of every installed Adapter plugin: whether it is the
    /// local deterministic adapter, configured/enabled, and whether its binary is
    /// present on PATH (`docs/RELUX_MASTER_PLAN.md` section 8.1, section 20.4).
    /// Probes PATH read-only; sorted by plugin id for deterministic listing.
    pub fn adapter_runtime_status(&self) -> Vec<AdapterRuntimeStatus> {
        let mut out: Vec<AdapterRuntimeStatus> = Vec::new();
        for installed in self.installed_plugins() {
            let Some(manifest) = self.plugins.get(&installed.id) else {
                continue;
            };
            if manifest.kind != PluginKind::Adapter {
                continue;
            }
            out.push(self.adapter_status_for(&installed.id, &manifest.name));
        }
        out.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
        out
    }

    /// Build the [`AdapterRuntimeStatus`] for one adapter plugin.
    fn adapter_status_for(&self, plugin_id: &PluginId, adapter_name: &str) -> AdapterRuntimeStatus {
        let recognized = recognize_adapter_kind(plugin_id.as_str());
        let config = self.adapter_runtime_configs.get(plugin_id);

        // The local deterministic adapter is always usable and never has a CLI.
        if recognized == Some(AdapterKind::LocalPrime) {
            return AdapterRuntimeStatus {
                plugin_id: plugin_id.as_str().to_string(),
                adapter_name: adapter_name.to_string(),
                kind: Some(AdapterKind::LocalPrime.as_str().to_string()),
                configured: false,
                enabled: false,
                command: None,
                available_on_path: false,
                resolved_path: None,
                timeout_seconds: None,
                max_output_bytes: None,
                working_dir: None,
                state: AdapterRuntimeState::LocalDeterministic,
                detail: "Local deterministic Prime adapter (echo path); always available, no CLI."
                    .to_string(),
            };
        }

        let kind = config
            .map(|c| c.kind.clone())
            .or_else(|| recognized.clone());
        let command = config
            .and_then(|c| c.resolved_command())
            .or_else(|| recognized.as_ref().and_then(|k| k.default_command().map(str::to_string)));
        let resolved_path = command
            .as_ref()
            .and_then(|bin| crate::adapter::find_on_path(bin));
        let available_on_path = resolved_path.is_some();
        let enabled = config.map(|c| c.enabled).unwrap_or(false);

        let (state, detail) = if let Some(cfg) = config {
            if !cfg.enabled {
                (
                    AdapterRuntimeState::Disabled,
                    "Runtime configured but disabled. Enable it to run assigned tasks."
                        .to_string(),
                )
            } else if available_on_path {
                (
                    AdapterRuntimeState::Available,
                    format!(
                        "Enabled. Relux will run '{}' for assigned tasks.",
                        command.clone().unwrap_or_default()
                    ),
                )
            } else {
                (
                    AdapterRuntimeState::MissingBinary,
                    format!(
                        "Enabled, but the binary '{}' was not found on PATH.",
                        command.clone().unwrap_or_default()
                    ),
                )
            }
        } else {
            (
                AdapterRuntimeState::NeedsConfiguration,
                "No runtime configured. CLI adapters are disabled by default; enable it to use it."
                    .to_string(),
            )
        };

        AdapterRuntimeStatus {
            plugin_id: plugin_id.as_str().to_string(),
            adapter_name: adapter_name.to_string(),
            kind: kind.as_ref().map(|k| k.as_str().to_string()),
            configured: config.is_some(),
            enabled,
            command,
            available_on_path,
            resolved_path: resolved_path.map(|p| p.display().to_string()),
            timeout_seconds: config.map(|c| c.timeout_seconds),
            max_output_bytes: config.map(|c| c.max_output_bytes),
            working_dir: config.and_then(|c| c.working_dir.clone()),
            state,
            detail,
        }
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

    /// Grant a permission to an existing agent.
    pub fn grant_permission_to_agent(
        &mut self,
        agent_id: &AgentId,
        permission: Permission,
    ) -> Result<(), KernelError> {
        let agent = self
            .agents
            .get_mut(agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?;

        if agent.permissions.contains(&permission) {
            return Err(KernelError::PermissionAlreadyGranted(
                agent_id.to_string(),
                permission.to_string(),
            ));
        }

        agent.permissions.push(permission.clone());
        let namespace_id = agent.namespace_id.clone();

        self.record_audit(
            "kernel",
            "kernel",
            "agent:grant_permission",
            Some("agent"),
            Some(agent_id.as_str()),
            Some(&namespace_id),
            AuditResult::Success,
            serde_json::json!({ "permission": permission.as_str() }),
        );

        Ok(())
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

    /// Returns the AgentId of the Prime agent, if it exists.
    pub fn prime_agent_id(&self) -> Option<AgentId> {
        self.agents.values().find(|a| a.id.as_str() == "prime").map(|a| a.id.clone())
    }

    /// Executes one safe tick of Prime's autonomy loop.
    pub fn one_autonomy_tick(&mut self) -> PrimeAutonomyTickResult {
        let mut result = PrimeAutonomyTickResult {
            tick_at: self.clock.tick(),
            ..Default::default()
        };
        let config = self.prime_autonomy_config.clone();
        let max_tasks = config.max_tasks_per_tick.max(1) as usize;

        if !config.enabled {
            result.summary = "Autonomy is disabled.".to_string();
            self.finish_autonomy_tick(&result);
            self.record_audit(
                "kernel",
                "prime",
                "autonomy:tick_skipped",
                None,
                None,
                None,
                AuditResult::Denied,
                serde_json::json!({ "reason": "autonomy disabled" }),
            );
            return result;
        }

        let prime_agent_id = match self.prime_agent_id() {
            Some(id) => id,
            None => {
                result.summary = "Prime agent not found.".to_string();
                self.finish_autonomy_tick(&result);
                self.record_audit(
                    "kernel",
                    "prime",
                    "autonomy:tick_skipped",
                    None,
                    None,
                    None,
                    AuditResult::Denied,
                    serde_json::json!({ "reason": "prime agent not found" }),
                );
                return result;
            }
        };

        let mut candidates: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| {
                matches!(task.status, TaskStatus::Created | TaskStatus::Queued)
                    && task.assigned_agent.is_some()
            })
            .map(|task| task.id.clone())
            .collect();
        candidates.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        if config.auto_assign_unassigned && candidates.len() < max_tasks {
            let mut unassigned: Vec<TaskId> = self
                .tasks
                .values()
                .filter(|task| {
                    matches!(task.status, TaskStatus::Created | TaskStatus::Queued)
                        && task.assigned_agent.is_none()
                })
                .map(|task| task.id.clone())
                .collect();
            unassigned.sort_by(|a, b| a.as_str().cmp(b.as_str()));

            for task_id in unassigned {
                if candidates.len() >= max_tasks {
                    break;
                }
                let namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());
                match self.assign_task(&task_id, &prime_agent_id) {
                    Ok(()) => {
                        result.tasks_assigned += 1;
                        result.actions_taken += 1;
                        candidates.push(task_id.clone());
                        self.record_audit(
                            "kernel",
                            "prime",
                            "autonomy:task_assigned",
                            Some("task"),
                            Some(task_id.as_str()),
                            namespace.as_ref(),
                            AuditResult::Success,
                            serde_json::json!({ "agent": prime_agent_id.as_str() }),
                        );
                    }
                    Err(e) => {
                        let reason = format!("Failed to auto-assign task {task_id}: {e}");
                        result.skipped_reasons.push(reason.clone());
                        self.record_audit(
                            "kernel",
                            "prime",
                            "autonomy:task_assign_failed",
                            Some("task"),
                            Some(task_id.as_str()),
                            namespace.as_ref(),
                            AuditResult::Failed,
                            serde_json::json!({ "reason": reason }),
                        );
                    }
                }
            }
        }

        for task_id in candidates.into_iter().take(max_tasks) {
            let namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());
            let status = self.tasks.get(&task_id).map(|t| t.status.clone());
            if matches!(status, Some(TaskStatus::Created | TaskStatus::Queued)) {
                if let Err(e) = self.start_run(&task_id) {
                    let reason = format!("Failed to start run for task {task_id}: {e}");
                    result.skipped_reasons.push(reason.clone());
                    self.record_audit(
                        "kernel",
                        "prime",
                        "autonomy:run_start_failed",
                        Some("task"),
                        Some(task_id.as_str()),
                        namespace.as_ref(),
                        AuditResult::Failed,
                        serde_json::json!({ "reason": reason }),
                    );
                    continue;
                }
            }

            match self.execute_local_run(&task_id) {
                Ok(run_id) => {
                    result.tasks_run += 1;
                    result.actions_taken += 1;
                    self.record_audit(
                        "kernel",
                        "prime",
                        "autonomy:run_completed",
                        Some("run"),
                        Some(run_id.as_str()),
                        namespace.as_ref(),
                        AuditResult::Success,
                        serde_json::json!({ "task": task_id.as_str() }),
                    );
                }
                Err(e) => {
                    let reason = format!("Failed to execute task {task_id}: {e}");
                    result.skipped_reasons.push(reason.clone());
                    self.record_audit(
                        "kernel",
                        "prime",
                        "autonomy:run_failed",
                        Some("task"),
                        Some(task_id.as_str()),
                        namespace.as_ref(),
                        AuditResult::Failed,
                        serde_json::json!({ "reason": reason }),
                    );
                }
            }
        }

        if result.actions_taken == 0 {
            result.summary = "No safe assigned work was ready for Prime autonomy.".to_string();
        } else {
            result.summary = format!(
                "{} task(s) run, {} task(s) assigned.",
                result.tasks_run, result.tasks_assigned
            );
        }
        if !result.skipped_reasons.is_empty() {
            result
                .summary
                .push_str(&format!(" Skipped: {}", result.skipped_reasons.join("; ")));
        }

        self.finish_autonomy_tick(&result);
        result
    }

    fn finish_autonomy_tick(&mut self, result: &PrimeAutonomyTickResult) {
        self.prime_autonomy_config.last_tick_at = Some(result.tick_at.clone());
        self.prime_autonomy_config.last_tick_summary = Some(result.summary.clone());
    }

    // --- Orchestration (multi-agent autonomy) ------------------------------
    //
    // The first slice of Prime-as-orchestrator (`docs/RELUX_MASTER_PLAN.md`
    // section 10.4 Delegation Rules, section 15 "real multi-agent workloads").
    // Planning is the pure brain ([`relux_core::plan_orchestration`]); the kernel
    // turns a multi-agent plan into real briefs and later runs them in a governed
    // batch through each agent's own adapter.

    /// All orchestrations, sorted by id for deterministic listing.
    pub fn orchestrations(&self) -> Vec<&Orchestration> {
        let mut out: Vec<&Orchestration> = self.orchestrations.values().collect();
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        out
    }

    pub fn orchestration(&self, id: &OrchestrationId) -> Option<&Orchestration> {
        self.orchestrations.get(id)
    }

    pub fn orchestration_count(&self) -> usize {
        self.orchestrations.len()
    }

    /// Decompose a goal into role-typed briefs, assign each to a fitting agent (or
    /// Prime when no specialist exists), and record the durable goal -> brief ->
    /// agent link. Creates and assigns work but does NOT run it - running is the
    /// separate governed [`run_orchestration`] batch, so nothing executes (and no
    /// paid CLI is ever spawned) without an explicit start (section 10.4).
    pub fn prime_orchestrate(
        &mut self,
        ctx: &PrimeContext,
        goal: &str,
    ) -> Result<Orchestration, KernelError> {
        let summary = self.inspect_state();
        let plan = plan_orchestration(goal, &summary);
        if !plan.is_multi_agent() {
            return Err(KernelError::OrchestrationNotMultiAgent);
        }

        self.next_orchestration += 1;
        let orch_id = OrchestrationId::new(format!("orch_{:04}", self.next_orchestration));
        let now = self.clock.tick();

        // Prime is the fallback assignee for any role without a specialist on the
        // roster. It always exists once bootstrapped; if somehow absent, fall back
        // to the acting agent so a brief is never left unassigned.
        let prime_fallback = self.prime_agent_id().unwrap_or_else(|| ctx.agent.clone());

        let mut steps: Vec<OrchestrationStep> = Vec::new();
        for planned in &plan.steps {
            let agent_id = planned
                .agent_id
                .as_ref()
                .map(|id| AgentId::new(id.clone()))
                .filter(|id| self.agents.contains_key(id))
                .unwrap_or_else(|| prime_fallback.clone());

            let task_id = self.create_task(
                &planned.title,
                serde_json::json!({
                    "orchestration": orch_id.as_str(),
                    "goal": plan.goal,
                    "role": planned.role.label(),
                }),
                &ctx.actor,
                &ctx.namespace,
                vec![],
            );
            self.assign_task(&task_id, &agent_id)?;
            steps.push(OrchestrationStep {
                task_id,
                agent_id,
                role: planned.role,
                title: planned.title.clone(),
                outcome: StepOutcome::Pending,
                // The planner's inferred dependencies index into `plan.steps`, and
                // briefs are committed in that same order, so the indices carry
                // over unchanged.
                depends_on: planned.depends_on.clone(),
                run_id: None,
                note: None,
                started_at: None,
                finished_at: None,
                round: None,
            });
        }

        let record = Orchestration {
            id: orch_id.clone(),
            goal: plan.goal.clone(),
            created_by: ctx.actor.clone(),
            namespace_id: ctx.namespace.clone(),
            status: OrchestrationStatus::Planned,
            steps,
            notes: plan.notes.clone(),
            created_at: now.clone(),
            updated_at: now,
            last_batch_summary: None,
        };
        let agent_ids: Vec<&str> = record.steps.iter().map(|s| s.agent_id.as_str()).collect();
        self.record_audit(
            "agent",
            ctx.agent.as_str(),
            "orchestration:create",
            Some("orchestration"),
            Some(orch_id.as_str()),
            Some(&ctx.namespace),
            AuditResult::Success,
            serde_json::json!({
                "goal": record.goal,
                "briefs": record.steps.len(),
                "agents": agent_ids,
            }),
        );
        self.orchestrations.insert(orch_id.clone(), record.clone());
        Ok(record)
    }

    /// Run a governed, dependency-aware multi-agent batch for one orchestration.
    ///
    /// The scheduler works in **rounds**. Each round it marks any brief whose
    /// dependency failed/blocked as `Blocked` (honestly, never run), collects the
    /// briefs that are *ready* (still pending and every dependency `Completed`),
    /// and runs up to `concurrency` of them (clamped 1..=4). It repeats until no
    /// brief is ready or the per-call budget `max` (clamped 1..=25) is spent.
    ///
    /// Each ready brief is executed through ITS assigned agent's adapter via the
    /// same governed path as the Work page ([`execute_assigned_run`]): the local
    /// Prime adapter echoes deterministically; an **enabled** CLI adapter spawns
    /// its binary; a disabled/unconfigured runtime or a missing permission is
    /// recorded as `blocked`, never faked. The loop runs each brief at most once,
    /// records per-brief start/finish/round, updates the durable record, and stops
    /// safely - it never loops forever, recurses, or auto-runs downloaded plugin
    /// code (section 10.4, section 8.1, section 8.2).
    ///
    /// Termination + no-deadlock are structural: dependencies only ever point at
    /// earlier briefs (a DAG), so after the block-propagation the lowest-index
    /// pending brief always has all dependencies `Completed` (it is ready) - the
    /// ready set is empty only when no brief is pending. Every round runs at least
    /// one brief and moves it to a terminal outcome, so the pending set strictly
    /// shrinks.
    ///
    /// Concurrency note: this synchronous entry point now performs **true bounded
    /// OS-parallel execution** — the independent briefs ready in one round run as
    /// real concurrent adapter processes (up to `concurrency` at once), exactly like
    /// the non-blocking job path. Both paths drive the SAME scheduler/executor
    /// primitives — [`prepare_orchestration_round`] (schedule + start + inline
    /// resolve), [`run_briefs_in_parallel`] (off-lock OS-thread spawn), and
    /// [`finalize_prepared_brief`] (merge each result) — so there is one execution
    /// semantics, not two. The difference is only the surrounding harness: this
    /// synchronous driver owns the kernel exclusively (a one-shot CLI process, the
    /// blocking `/run` API handler under its lock, or a test), so there is no lock to
    /// release between phases and no per-round persistence; the HTTP server wraps the
    /// same three phases with lock-release + persist-between-rounds so a concurrent
    /// poll stays responsive. It is used by the blocking `POST .../run` API and the
    /// `prime orchestration run` CLI, which therefore get real parallelism too.
    pub fn run_orchestration(
        &mut self,
        id: &OrchestrationId,
        max: usize,
        concurrency: usize,
    ) -> Result<OrchestrationBatchResult, KernelError> {
        let max = max.clamp(1, 25);
        let concurrency = concurrency.clamp(1, 4);
        let mut result = self.new_orchestration_batch_result(id, concurrency)?;

        // Drive the rounds to completion through the shared prepare -> spawn-parallel
        // -> finalize engine (the same primitives the background job path drives one
        // round at a time). Each round:
        //   1. prepare (schedule the ready set within the `max` budget, start each
        //      brief's run, resolve local-echo / pre-spawn-blocked briefs inline, and
        //      return the enabled-CLI briefs as spawn plans),
        //   2. run those prepared adapter processes on real OS threads concurrently,
        //      bounded by the round's concurrency cap,
        //   3. finalize each finished brief back into the durable record.
        // The kernel is owned exclusively here, so the phases run back-to-back with no
        // lock to release. A failure or panic in one brief's thread never corrupts a
        // sibling: each owns its own run/task records and is merged independently.
        let mut round_no: u32 = 0;
        loop {
            let next_round = round_no + 1;
            let prep =
                self.prepare_orchestration_round(id, max, concurrency, next_round, &mut result)?;
            // An empty prep means the `max` budget is spent or no brief is ready — the
            // batch is complete. (Termination is structural: every non-empty round
            // moves >=1 brief to a terminal outcome, so the pending set strictly
            // shrinks.)
            if !prep.ran() {
                break;
            }
            let finished = run_briefs_in_parallel(prep.prepared);
            for f in finished {
                self.finalize_prepared_brief(id, f, &mut result);
            }
            round_no = next_round;
        }
        result.rounds = round_no;
        self.finalize_orchestration_batch(id, &mut result)?;
        Ok(result)
    }

    /// Build the fresh, empty [`OrchestrationBatchResult`] accumulator for a batch,
    /// validating that the orchestration exists. Its status starts `Planned`;
    /// [`finalize_orchestration_batch`] recomputes the real status from the steps.
    /// The non-blocking job path uses this to own the accumulator across the
    /// lock-released rounds it drives.
    pub fn new_orchestration_batch_result(
        &self,
        id: &OrchestrationId,
        concurrency: usize,
    ) -> Result<OrchestrationBatchResult, KernelError> {
        let concurrency = concurrency.clamp(1, 4);
        self.orchestrations
            .get(id)
            .ok_or_else(|| KernelError::UnknownOrchestration(id.to_string()))?;
        Ok(OrchestrationBatchResult {
            orchestration_id: id.clone(),
            ran: 0,
            completed: 0,
            failed: 0,
            blocked: 0,
            pending: 0,
            concurrency: concurrency as u32,
            rounds: 0,
            waiting: 0,
            dependency_blocked: 0,
            skipped_reasons: Vec::new(),
            per_agent: Vec::new(),
            summary: String::new(),
            next_action: String::new(),
            status: OrchestrationStatus::Planned,
        })
    }

    /// Merge one brief's execution result into the batch tallies and its durable
    /// step record. Shared by the inline-resolved briefs in
    /// [`prepare_orchestration_round`] and the off-lock briefs merged in
    /// [`finalize_prepared_brief`] so both interpret an `execute`/`finalize` outcome identically:
    /// `Ok` is completed; a runtime/permission/binary error is `Blocked` (needs a
    /// human); any other error is `Failed` (retryable). The latest run for the task
    /// is the attempt just made, so the step's `run_id` points at it.
    #[allow(clippy::too_many_arguments)]
    fn record_brief_outcome(
        &mut self,
        id: &OrchestrationId,
        idx: usize,
        round_no: u32,
        agent_label: &str,
        task_id: &TaskId,
        exec: Result<RunId, KernelError>,
        started_at: String,
        finished_at: String,
        result: &mut OrchestrationBatchResult,
    ) {
        // The latest run for this task is the attempt we just made (run ids are
        // zero-padded, so id order == creation order).
        let run_id = self
            .runs
            .values()
            .filter(|r| r.task_id == *task_id)
            .max_by(|a, b| a.id.0.cmp(&b.id.0))
            .map(|r| r.id.clone());
        let (outcome, note) = match exec {
            Ok(_) => {
                result.completed += 1;
                (StepOutcome::Completed, None)
            }
            Err(e) => {
                // Distinguish "needs a human" (blocked) from a genuine run
                // failure (retryable). A disabled/unconfigured CLI runtime, a
                // missing binary, an invalid config, or a missing permission
                // all mean the brief cannot run until someone acts.
                let blocked = matches!(
                    e,
                    KernelError::AdapterRuntimeDisabled { .. }
                        | KernelError::AdapterRuntimeNotConfigured { .. }
                        | KernelError::InvalidAdapterConfig { .. }
                        | KernelError::AdapterBinaryMissing { .. }
                        | KernelError::PermissionDenied { .. }
                );
                if blocked {
                    result.blocked += 1;
                } else {
                    result.failed += 1;
                }
                let reason = e.to_string();
                result.skipped_reasons.push(format!("{task_id}: {reason}"));
                (
                    if blocked {
                        StepOutcome::Blocked
                    } else {
                        StepOutcome::Failed
                    },
                    Some(reason),
                )
            }
        };
        result
            .per_agent
            .push(format!("round {round_no} {agent_label}: {task_id} {}", outcome.label()));
        if let Some(o) = self.orchestrations.get_mut(id) {
            if let Some(step) = o.steps.get_mut(idx) {
                step.outcome = outcome;
                step.run_id = run_id;
                step.note = note;
                step.started_at = Some(started_at);
                step.finished_at = Some(finished_at);
                step.round = Some(round_no);
            }
        }
    }

    /// Prepare ONE dependency-aware round for OS-parallel execution, under the
    /// kernel lock. Mirrors [`run_one_orchestration_round`]'s scheduling exactly —
    /// propagate dependency blocks, collect the ready set in index order, take up to
    /// `concurrency` within the `max` budget — but instead of spawning each brief's
    /// CLI inline it:
    ///
    /// - runs local-echo briefs inline (deterministic, no blocking I/O) and records
    ///   their outcome immediately;
    /// - records inline any brief that blocks/fails before a spawn (disabled
    ///   runtime, missing binary, permission denied, no active run);
    /// - for an enabled CLI brief, prepares the spawn ([`prepare_cli_run`]), stamps
    ///   the step's `run_id`/`started_at`/`round` so a mid-flight poll sees it, and
    ///   returns it as a [`PreparedBrief`] to run off-lock.
    ///
    /// The caller releases the lock, runs every [`PreparedBrief`] in parallel, then
    /// calls [`finalize_prepared_brief`] for each under the lock. `result.ran` is
    /// incremented here for every attempted brief (inline or prepared), so the
    /// budget is honoured across the split. Returns the [`RoundPrep`]; an empty,
    /// non-`ran` result means no brief was ready (the batch is complete).
    pub fn prepare_orchestration_round(
        &mut self,
        id: &OrchestrationId,
        max: usize,
        concurrency: usize,
        round_no: u32,
        result: &mut OrchestrationBatchResult,
    ) -> Result<RoundPrep, KernelError> {
        let max = max.clamp(1, 25);
        let concurrency = concurrency.clamp(1, 4);
        let mut prep = RoundPrep {
            ran_inline: 0,
            prepared: Vec::new(),
        };

        if result.ran as usize >= max {
            return Ok(prep);
        }

        result.dependency_blocked += self.propagate_dependency_blocks(id);

        let ready: Vec<(usize, TaskId, AgentId)> = self
            .orchestrations
            .get(id)
            .map(|o| {
                o.steps
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| {
                        s.outcome == StepOutcome::Pending
                            && s.depends_on.iter().all(|&j| {
                                o.steps
                                    .get(j)
                                    .map(|d| d.outcome == StepOutcome::Completed)
                                    .unwrap_or(true)
                            })
                    })
                    .map(|(i, s)| (i, s.task_id.clone(), s.agent_id.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if ready.is_empty() {
            return Ok(prep);
        }

        let budget_left = max - result.ran as usize;
        let take = concurrency.min(budget_left);
        for (idx, task_id, agent_id) in ready.into_iter().take(take) {
            result.ran += 1;
            let started_at = self.clock.tick();

            // Resolve the adapter kind for this brief's assigned agent. A brief that
            // cannot even be looked up is recorded as a failure inline (never spawned).
            let adapter = self
                .agents
                .get(&agent_id)
                .map(|a| a.adapter_plugin.clone());
            let kind = adapter.as_ref().map(|a| recognize_adapter_kind(a.as_str()));

            // Start the run if the task is still Created/Queued, exactly like
            // execute_assigned_run. A start failure (e.g. permission) is recorded
            // inline as a block/fail.
            let needs_start = self
                .tasks
                .get(&task_id)
                .map(|t| matches!(t.status, TaskStatus::Created | TaskStatus::Queued))
                .unwrap_or(false);
            if needs_start {
                if let Err(e) = self.start_run(&task_id) {
                    let finished_at = self.clock.tick();
                    self.record_brief_outcome(
                        id, idx, round_no, agent_id.as_str(), &task_id, Err(e), started_at,
                        finished_at, result,
                    );
                    prep.ran_inline += 1;
                    continue;
                }
            }

            match (adapter, kind) {
                // Local Prime echo: deterministic, no blocking I/O — run under the
                // lock and record immediately.
                (Some(_), Some(Some(AdapterKind::LocalPrime))) => {
                    let exec = self.execute_local_run(&task_id);
                    let finished_at = self.clock.tick();
                    self.record_brief_outcome(
                        id, idx, round_no, agent_id.as_str(), &task_id, exec, started_at,
                        finished_at, result,
                    );
                    prep.ran_inline += 1;
                }
                // An enabled CLI adapter: prepare the spawn, then run it off-lock.
                (Some(adapter_id), _) => match self.prepare_cli_run(&task_id, &adapter_id) {
                    Ok(plan) => {
                        // Make the in-flight brief visible to a mid-round poll: the
                        // run exists and is Running; stamp the durable step now.
                        let run_id = plan.run_id.clone();
                        if let Some(o) = self.orchestrations.get_mut(id) {
                            if let Some(step) = o.steps.get_mut(idx) {
                                step.run_id = Some(run_id);
                                step.started_at = Some(started_at.clone());
                                step.round = Some(round_no);
                            }
                        }
                        prep.prepared.push(PreparedBrief {
                            step_index: idx,
                            round_no,
                            agent_label: agent_id.as_str().to_string(),
                            started_at,
                            plan,
                        });
                    }
                    Err(e) => {
                        let finished_at = self.clock.tick();
                        self.record_brief_outcome(
                            id, idx, round_no, agent_id.as_str(), &task_id, Err(e), started_at,
                            finished_at, result,
                        );
                        prep.ran_inline += 1;
                    }
                },
                // No adapter / unknown agent: an honest failure, recorded inline.
                (None, _) => {
                    let exec = self.execute_assigned_run(&task_id);
                    let finished_at = self.clock.tick();
                    self.record_brief_outcome(
                        id, idx, round_no, agent_id.as_str(), &task_id, exec, started_at,
                        finished_at, result,
                    );
                    prep.ran_inline += 1;
                }
            }
        }
        Ok(prep)
    }

    /// Finalize one [`FinishedBrief`] (its adapter process ran off-lock) back into
    /// the orchestration record under the kernel lock: parse + record the output,
    /// complete/fail the run + task, then merge the outcome and tallies via
    /// [`record_brief_outcome`]. The step's `started_at`/`round` were already stamped
    /// at prepare time; this sets the terminal `outcome`, `finished_at`, and note.
    pub fn finalize_prepared_brief(
        &mut self,
        id: &OrchestrationId,
        finished: FinishedBrief,
        result: &mut OrchestrationBatchResult,
    ) {
        let FinishedBrief {
            step_index,
            round_no,
            agent_label,
            started_at,
            plan,
            outcome,
        } = finished;
        let task_id = plan.task_id.clone();
        let exec = self.finalize_cli_run(plan, outcome);
        let finished_at = self.clock.tick();
        self.record_brief_outcome(
            id,
            step_index,
            round_no,
            &agent_label,
            &task_id,
            exec,
            started_at,
            finished_at,
            result,
        );
    }

    /// Finalize a batch: recompute the orchestration's status/pending/waiting from
    /// the full step set, fill the summary + next action, update the durable record,
    /// and write the audit entry. Called once after the last round by both the
    /// synchronous [`run_orchestration`] and the non-blocking job. `result.rounds`
    /// must already be set by the caller.
    pub fn finalize_orchestration_batch(
        &mut self,
        id: &OrchestrationId,
        result: &mut OrchestrationBatchResult,
    ) -> Result<(), KernelError> {
        let namespace = self
            .orchestrations
            .get(id)
            .ok_or_else(|| KernelError::UnknownOrchestration(id.to_string()))?
            .namespace_id
            .clone();

        // A brief whose dependency failed in the final round must be marked blocked
        // too (the loop may have exited on the budget guard before re-propagating).
        result.dependency_blocked += self.propagate_dependency_blocks(id);

        // Recompute overall status, pending, and dependency-waiting from the full
        // step set.
        let (status, pending_left, waiting) = {
            let o = self
                .orchestrations
                .get(id)
                .ok_or_else(|| KernelError::UnknownOrchestration(id.to_string()))?;
            let pending_left = o
                .steps
                .iter()
                .filter(|s| s.outcome == StepOutcome::Pending)
                .count();
            // Pending briefs still gated by a dependency that has not completed
            // (they will become runnable once their upstream finishes).
            let waiting = o
                .steps
                .iter()
                .filter(|s| {
                    s.outcome == StepOutcome::Pending
                        && s.depends_on.iter().any(|&j| {
                            o.steps
                                .get(j)
                                .map(|d| d.outcome != StepOutcome::Completed)
                                .unwrap_or(false)
                        })
                })
                .count();
            let any_blocked = o.steps.iter().any(|s| s.outcome == StepOutcome::Blocked);
            let any_failed = o.steps.iter().any(|s| s.outcome == StepOutcome::Failed);
            let status = if pending_left > 0 {
                OrchestrationStatus::Running
            } else if any_blocked || any_failed {
                OrchestrationStatus::NeedsAttention
            } else {
                OrchestrationStatus::Completed
            };
            (status, pending_left, waiting)
        };
        result.pending = pending_left as u32;
        result.waiting = waiting as u32;
        result.status = status;
        result.summary = format!(
            "{} round(s), up to {} brief(s) at a time: {} ran ({} completed, {} failed, {} blocked); {} blocked by a failed dependency; {} waiting on a dependency; {} pending.",
            result.rounds,
            result.concurrency,
            result.ran,
            result.completed,
            result.failed,
            result.blocked,
            result.dependency_blocked,
            result.waiting,
            result.pending
        );
        result.next_action = match status {
            OrchestrationStatus::Completed => "All briefs completed. Review the runs.".to_string(),
            OrchestrationStatus::Running => {
                if result.waiting > 0 {
                    format!(
                        "{} brief(s) pending ({} waiting on a dependency that has not completed). Run the orchestration again to continue.",
                        result.pending, result.waiting
                    )
                } else {
                    format!(
                        "Run the orchestration again to continue {} remaining brief(s).",
                        result.pending
                    )
                }
            }
            OrchestrationStatus::NeedsAttention => format!(
                "{} brief(s) need attention. Blocked briefs need their adapter runtime enabled, an upstream brief retried, or reassignment; failed briefs can be retried. Then run again.",
                result.blocked + result.failed + result.dependency_blocked
            ),
            OrchestrationStatus::Planned => {
                "Run the orchestration to start the briefs.".to_string()
            }
        };

        let now = self.clock.tick();
        if let Some(o) = self.orchestrations.get_mut(id) {
            o.status = status;
            o.updated_at = now;
            o.last_batch_summary = Some(result.summary.clone());
        }
        self.record_audit(
            "agent",
            "prime",
            "orchestration:batch",
            Some("orchestration"),
            Some(id.as_str()),
            Some(&namespace),
            if result.failed > 0 || result.blocked > 0 || result.dependency_blocked > 0 {
                AuditResult::Failed
            } else {
                AuditResult::Success
            },
            serde_json::json!({
                "ran": result.ran,
                "completed": result.completed,
                "failed": result.failed,
                "blocked": result.blocked,
                "dependency_blocked": result.dependency_blocked,
                "rounds": result.rounds,
                "concurrency": result.concurrency,
                "waiting": result.waiting,
                "pending": result.pending,
            }),
        );
        Ok(())
    }

    /// Mark every still-pending brief whose dependency `Failed`/`Blocked` as
    /// `Blocked` itself, with an honest note pointing at the upstream brief.
    /// Iterates to a fixpoint so a block cascades down a chain in one pass.
    /// Returns the number of briefs newly blocked. Safe to call repeatedly (an
    /// already-terminal brief is never re-touched).
    fn propagate_dependency_blocks(&mut self, id: &OrchestrationId) -> u32 {
        let mut newly_blocked = 0u32;
        loop {
            let updates: Vec<(usize, String)> = match self.orchestrations.get(id) {
                Some(o) => o
                    .steps
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.outcome == StepOutcome::Pending)
                    .filter_map(|(i, s)| {
                        s.depends_on
                            .iter()
                            .find(|&&j| {
                                o.steps
                                    .get(j)
                                    .map(|d| {
                                        matches!(
                                            d.outcome,
                                            StepOutcome::Failed | StepOutcome::Blocked
                                        )
                                    })
                                    .unwrap_or(false)
                            })
                            .map(|&j| {
                                let dep = &o.steps[j];
                                (
                                    i,
                                    format!(
                                        "blocked: depends on {} which {}",
                                        dep.task_id,
                                        dep.outcome.label()
                                    ),
                                )
                            })
                    })
                    .collect(),
                None => return newly_blocked,
            };
            if updates.is_empty() {
                break;
            }
            for (i, reason) in updates {
                if let Some(o) = self.orchestrations.get_mut(id) {
                    if let Some(step) = o.steps.get_mut(i) {
                        if step.outcome == StepOutcome::Pending {
                            step.outcome = StepOutcome::Blocked;
                            step.note = Some(reason);
                            newly_blocked += 1;
                        }
                    }
                }
            }
        }
        newly_blocked
    }

    // --- Runs --------------------------------------------------------------

    /// Start an execution attempt for an assigned task
    /// (`docs/RELUX_MASTER_PLAN.md` section 9.6, section 13.6). The run inherits the assigned
    /// agent's adapter plugin and the task moves to `Running`.
    pub fn start_run(&mut self, task_id: &TaskId) -> Result<RunId, KernelError> {
        let (agent_id, namespace, required_permissions) = {
            let task = self
                .tasks
                .get(task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            let agent_id = task
                .assigned_agent
                .clone()
                .ok_or_else(|| KernelError::TaskNotAssigned(task_id.to_string()))?;
            (agent_id, task.namespace_id.clone(), task.required_permissions.clone())
        };

        let agent = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?;

        // --- Permission check: Agent must have all permissions required by the task ---
        for required_perm in &required_permissions {
            if !agent.permissions.iter().any(|p| p.matches_exact(required_perm)) {
                self.record_audit(
                    "agent",
                    agent_id.as_str(),
                    "task:start_run",
                    Some("run"),
                    None, // No run_id yet
                    Some(&namespace),
                    AuditResult::Denied,
                    serde_json::json!({
                        "task": task_id.as_str(),
                        "reason": format!("agent lacks required permission: {}", required_perm.as_str())
                    }),
                );
                return Err(KernelError::PermissionDenied {
                    agent: agent_id.to_string(),
                    permission: required_perm.to_string(),
                });
            }
        }
        // --- End permission check ---

        let adapter_plugin = agent.adapter_plugin.clone();

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
            duration_ms: None,
            usage: None,
            cost: None,
            retried_from: None,
            artifacts: Vec::new(),
            proposed_changes: Vec::new(),
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

    /// Route a tool call from an agent through the kernel, inside a run
    /// (`docs/RELUX_MASTER_PLAN.md` section 13.6, section 10.2).
    ///
    /// The kernel resolves the tool on the named plugin, looks up the permission
    /// it requires, and verifies the agent holds it. Denials and successes are
    /// both audited and recorded on the run transcript.
    ///
    /// This is honest about what the local runtime can actually do: only the
    /// kernel's built-in deterministic handlers (`crate::builtin`) execute. An
    /// installed-but-unimplemented tool is refused with
    /// [`KernelError::ToolRuntimeUnavailable`] - audited as a failure, recorded on
    /// the transcript, and never fabricating an output. Arbitrary downloaded
    /// plugin code is not executed (master plan section 8.2).
    pub fn call_tool(
        &mut self,
        run_id: &RunId,
        agent_id: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, KernelError> {
        let (namespace, required) = self.resolve_tool_permission(agent_id, plugin_id, tool_name)?;

        if !self.agent_holds_permission(agent_id, &required) {
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

        // Execute via a built-in deterministic handler or the plugin's configured
        // HTTP loopback runtime. Any failure (unconfigured, disabled, or a
        // loopback error) is audited and recorded on the transcript - no
        // fabricated output. Arbitrary downloaded plugin code is never executed.
        let output = match self.execute_tool_runtime(plugin_id, tool_name, &input) {
            Ok(output) => output,
            Err(e) => {
                self.push_run_event(
                    run_id,
                    "tool_call_failed",
                    "kernel",
                    &format!("{tool_name} on {plugin_id} did not run: {e}"),
                    serde_json::json!({ "tool": tool_name, "plugin": plugin_id.as_str() }),
                );
                self.record_audit(
                    "agent",
                    agent_id.as_str(),
                    required.as_str(),
                    Some("tool"),
                    Some(tool_name),
                    Some(&namespace),
                    AuditResult::Failed,
                    serde_json::json!({ "run": run_id.as_str(), "reason": e.to_string() }),
                );
                return Err(e);
            }
        };

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

    /// Invoke a tool through the kernel OUTSIDE a run
    /// (`docs/RELUX_MASTER_PLAN.md` section 13.6, section 10.2).
    ///
    /// This is the clean audit path behind the `/v1/relux/tools/invoke` endpoint
    /// and the `relux-kernel tool invoke` CLI: it runs the same permission check
    /// and built-in-runtime gate as [`call_tool`] but does not invent a run/run
    /// transcript. The invocation is recorded on the append-only audit log
    /// (success, denial, or not-implemented) so nothing bypasses the kernel, and
    /// the structured [`ToolInvocationResult`] carries only real output.
    pub fn invoke_tool(
        &mut self,
        agent_id: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<ToolInvocationResult, KernelError> {
        let (namespace, required) = self.resolve_tool_permission(agent_id, plugin_id, tool_name)?;

        if !self.agent_holds_permission(agent_id, &required) {
            self.record_audit(
                "agent",
                agent_id.as_str(),
                required.as_str(),
                Some("tool"),
                Some(tool_name),
                Some(&namespace),
                AuditResult::Denied,
                serde_json::json!({ "via": "invoke", "plugin": plugin_id.as_str() }),
            );
            return Err(KernelError::PermissionDenied {
                agent: agent_id.to_string(),
                permission: required.to_string(),
            });
        }

        let output = match self.execute_tool_runtime(plugin_id, tool_name, &input) {
            Ok(output) => output,
            Err(e) => {
                self.record_audit(
                    "agent",
                    agent_id.as_str(),
                    required.as_str(),
                    Some("tool"),
                    Some(tool_name),
                    Some(&namespace),
                    AuditResult::Failed,
                    serde_json::json!({
                        "via": "invoke",
                        "plugin": plugin_id.as_str(),
                        "reason": e.to_string()
                    }),
                );
                return Err(e);
            }
        };

        self.record_audit(
            "agent",
            agent_id.as_str(),
            required.as_str(),
            Some("tool"),
            Some(tool_name),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({ "via": "invoke", "plugin": plugin_id.as_str() }),
        );
        Ok(ToolInvocationResult {
            plugin_id: plugin_id.to_string(),
            tool_name: tool_name.to_string(),
            agent_id: agent_id.to_string(),
            permission: required.to_string(),
            output,
        })
    }

    /// Discover every installed plugin tool with its executable status
    /// (`docs/RELUX_MASTER_PLAN.md` section 7.4, `docs/Relux spec.md` section 20.2
    /// Tools view). Each descriptor marks `ready`, `not_implemented`, or - when
    /// `agent_for_permission` is supplied and that agent lacks the permission -
    /// `missing_permission`. Sorted by `(plugin_id, tool_name)` for deterministic
    /// listing. Never leaks config/secrets - only manifest-declared tool metadata.
    pub fn discover_tools(&self, agent_for_permission: Option<&AgentId>) -> Vec<ToolDescriptor> {
        let mut out: Vec<ToolDescriptor> = Vec::new();
        for installed in self.installed_plugins() {
            let Some(manifest) = self.plugins.get(&installed.id) else {
                continue;
            };
            let protected = installed.source_kind == PluginSourceKind::Bundled;
            let runtime = self.tool_runtime_configs.get(&installed.id);
            for tool in &manifest.capabilities.tools {
                let builtin = crate::builtin::is_builtin_tool(installed.id.as_str(), &tool.name);
                // A tool is runnable when a built-in handler exists OR the plugin
                // has an enabled HTTP loopback runtime configured. Otherwise the
                // status is honest about WHY it cannot run.
                let runnable = builtin || runtime.map(|c| c.enabled).unwrap_or(false);
                let executable = if runnable {
                    match agent_for_permission {
                        Some(agent_id) if !self.agent_holds_permission(agent_id, &tool.permission) => {
                            ToolExecutability::MissingPermission
                        }
                        _ => ToolExecutability::Ready,
                    }
                } else if runtime.is_some() {
                    // Configured but disabled.
                    ToolExecutability::RuntimeDisabled
                } else {
                    // No built-in handler and no runtime yet: the operator can
                    // make it runnable by configuring an HTTP loopback endpoint.
                    ToolExecutability::RuntimeNotConfigured
                };
                out.push(ToolDescriptor {
                    plugin_id: installed.id.as_str().to_string(),
                    tool_name: tool.name.clone(),
                    description: tool.description.clone(),
                    permission: tool.permission.as_str().to_string(),
                    risk: tool.risk.clone(),
                    source_kind: format!("{:?}", installed.source_kind),
                    installed: true,
                    enabled: installed.enabled,
                    protected,
                    executable,
                });
            }
        }
        out.sort_by(|a, b| {
            a.plugin_id
                .cmp(&b.plugin_id)
                .then_with(|| a.tool_name.cmp(&b.tool_name))
        });
        out
    }

    /// Resolve `(agent namespace, required permission)` for a tool call, erroring
    /// on an unknown agent, unknown plugin, or unknown tool. Shared by
    /// [`call_tool`] and [`invoke_tool`] so both gate identically.
    fn resolve_tool_permission(
        &self,
        agent_id: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
    ) -> Result<(NamespaceId, Permission), KernelError> {
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
        let required = manifest
            .capabilities
            .tools
            .iter()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| KernelError::ToolNotFound {
                plugin: plugin_id.to_string(),
                tool: tool_name.to_string(),
            })?
            .permission
            .clone();
        Ok((namespace, required))
    }

    /// True when `agent_id` exists and holds `permission` exactly.
    fn agent_holds_permission(&self, agent_id: &AgentId, permission: &Permission) -> bool {
        self.agents
            .get(agent_id)
            .map(|a| a.permissions.iter().any(|p| p.matches_exact(permission)))
            .unwrap_or(false)
    }

    /// Execute a tool through a supported runtime and return its output JSON.
    ///
    /// Resolution order: a built-in deterministic handler first, then the plugin's
    /// configured HTTP loopback runtime. Honest by construction:
    /// - no built-in and no runtime configured -> [`KernelError::ToolRuntimeUnavailable`]
    /// - a runtime is configured but disabled -> [`KernelError::ToolRuntimeDisabled`]
    /// - a loopback failure (connect/timeout/non-200/invalid-JSON/tool error) ->
    ///   [`KernelError::ToolRuntimeInvocation`]
    ///
    /// Arbitrary downloaded plugin code is never executed - only the operator's
    /// loopback server is ever called (`docs/RELUX_MASTER_PLAN.md` section 8.2,
    /// section 18).
    fn execute_tool_runtime(
        &self,
        plugin_id: &PluginId,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, KernelError> {
        if let Some(output) = self.builtin_tool_output(plugin_id.as_str(), tool_name, input) {
            return Ok(output);
        }
        match self.tool_runtime_configs.get(plugin_id) {
            None => Err(KernelError::ToolRuntimeUnavailable {
                plugin: plugin_id.to_string(),
                tool: tool_name.to_string(),
            }),
            Some(cfg) if !cfg.enabled => Err(KernelError::ToolRuntimeDisabled {
                plugin: plugin_id.to_string(),
            }),
            Some(cfg) => match cfg.kind {
                RuntimeKind::HttpLoopback => crate::runtime::invoke_http_loopback(
                    &cfg.base_url,
                    plugin_id.as_str(),
                    tool_name,
                    input,
                    cfg.timeout_ms,
                )
                .map_err(|e| KernelError::ToolRuntimeInvocation {
                    plugin: plugin_id.to_string(),
                    tool: tool_name.to_string(),
                    message: e.to_string(),
                }),
            },
        }
    }

    /// Execute a built-in deterministic tool handler, or `None` when the kernel
    /// has no runtime for the `(plugin, tool)` pair.
    ///
    /// Membership MUST match [`crate::builtin::BUILTIN_TOOLS`]. Every handler here
    /// is local-only and side-effect free: no network, no filesystem, no shelling
    /// out (master plan section 8.2 safety rules).
    fn builtin_tool_output(
        &self,
        plugin_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        match (plugin_id, tool_name) {
            // echo.say: return the input unchanged.
            ("relux-tools-echo", "echo.say") => Some(input.clone()),
            // status.summary: a deterministic snapshot of control-plane counts.
            ("relux-tools-status", "status.summary") => {
                Some(serde_json::to_value(self.inspect_state()).unwrap_or(serde_json::Value::Null))
            }
            _ => None,
        }
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

    /// Record the real measured metrics for a finished adapter run: wall-clock
    /// duration (always), and token usage + cost (only when the adapter emitted a
    /// structured result envelope we could parse). Master plan section 9.6.
    ///
    /// Best-effort: an unknown run id is a no-op (the caller already failed
    /// honestly). Never invents usage/cost it was not handed.
    fn set_run_metrics(
        &mut self,
        run_id: &RunId,
        duration_ms: u64,
        usage: Option<serde_json::Value>,
        cost: Option<f64>,
    ) {
        if let Some(run) = self.runs.get_mut(run_id) {
            run.duration_ms = Some(duration_ms);
            if usage.is_some() {
                run.usage = usage;
            }
            if cost.is_some() {
                run.cost = cost;
            }
        }
    }

    /// Persist the read-only artifact references an adapter declared in its
    /// structured result envelope onto the durable run record (master plan
    /// section 9.6 / section 15). Best-effort: an unknown run id or an empty set
    /// is a no-op (we never write an empty `artifacts` and never fabricate one).
    /// These are *references* only - capturing them does NOT enable apply.
    fn set_run_artifacts(&mut self, run_id: &RunId, artifacts: Vec<relux_core::RunArtifact>) {
        if artifacts.is_empty() {
            return;
        }
        if let Some(run) = self.runs.get_mut(run_id) {
            run.artifacts = artifacts;
        }
    }

    /// Persist the reviewable **proposed file changes** an adapter declared in its
    /// structured result envelope onto the durable run record (master plan section
    /// 15 / section 9.6). Best-effort: an unknown run id or an empty set is a no-op
    /// (we never write an empty `proposed_changes` and never fabricate one). These
    /// carry content and can be reviewed/applied, but capturing them NEVER applies
    /// them — apply requires an explicit operator approval + apply action.
    fn set_run_proposed_changes(
        &mut self,
        run_id: &RunId,
        changes: Vec<relux_core::ProposedChange>,
    ) {
        if changes.is_empty() {
            return;
        }
        if let Some(run) = self.runs.get_mut(run_id) {
            run.proposed_changes = changes;
        }
    }

    /// Record an operator's accept/reject of one proposed change (master plan
    /// section 15 review verdict). `approve = true` moves a `Proposed` change to
    /// `Approved` (eligible for an explicit apply); `false` moves it to `Rejected`
    /// (never applied). An already-`Applied` change cannot be re-reviewed. Returns
    /// the new status. Audited; never applies anything.
    pub fn review_proposed_change(
        &mut self,
        run_id: &RunId,
        index: usize,
        approve: bool,
        note: Option<&str>,
    ) -> Result<relux_core::ProposedChangeStatus, KernelError> {
        use relux_core::ProposedChangeStatus;
        let (agent_id, task_id) = {
            let run = self
                .runs
                .get(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            (run.agent_id.clone(), run.task_id.clone())
        };
        let task_namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());
        let new_status = {
            let run = self
                .runs
                .get_mut(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            let change = run.proposed_changes.get_mut(index).ok_or_else(|| {
                KernelError::UnknownProposedChange {
                    run: run_id.to_string(),
                    index,
                }
            })?;
            if change.status == ProposedChangeStatus::Applied {
                return Err(KernelError::ProposedChangeNotApplicable {
                    run: run_id.to_string(),
                    index,
                    reason: "the change is already applied and cannot be re-reviewed".to_string(),
                });
            }
            change.status = if approve {
                ProposedChangeStatus::Approved
            } else {
                ProposedChangeStatus::Rejected
            };
            if let Some(n) = note {
                change.set_review_note(n);
            }
            // A fresh decision clears a stale refusal reason from a prior attempt.
            change.refused_reason = None;
            change.status
        };
        self.record_audit(
            "agent",
            agent_id.as_str(),
            if approve {
                "proposed_change:approve"
            } else {
                "proposed_change:reject"
            },
            Some("run"),
            Some(run_id.as_str()),
            task_namespace.as_ref(),
            AuditResult::Success,
            serde_json::json!({ "index": index, "status": new_status.as_str() }),
        );
        Ok(new_status)
    }

    /// Apply one **approved** proposed change into the run's controlled workspace
    /// root (master plan section 15 apply, safety bar section 17.5). This is the
    /// one place the Relux kernel writes a file an agent proposed, so the bar is
    /// strict and every refusal is honest (no fabricated success):
    ///
    /// 1. the change must be in `Approved` state (approval required);
    /// 2. a `replace` must carry a `baseline_sha256` (v1 refuses without one — no
    ///    force); a `create` carries no baseline (there is no prior file);
    /// 3. the run's adapter must have a configured `working_dir` (the controlled
    ///    workspace root); otherwise there is nowhere safe to write;
    /// 4. the target resolves inside that root with no symlink/`..` escape and is
    ///    not an excluded (vcs/build/secret) path;
    /// 5. for a `replace` the target must already exist as a regular file whose
    ///    current SHA-256 equals the declared baseline (a mismatch is a
    ///    **conflict** — refused, the file untouched); for a `create` the target
    ///    must NOT already exist (an existing file is a **conflict** — never
    ///    overwritten), and any missing parent directories (each a safe component
    ///    inside the root, no symlink) are created;
    /// 6. only then is the new content written atomically (temp file + rename for a
    ///    replace; an O_EXCL reservation + temp + rename for a create, so a racing
    ///    creator never gets clobbered).
    ///
    /// On success the change flips to `Applied` (with an `applied_at` stamp), a
    /// transcript event + audit are recorded, and the applied path/bytes are
    /// returned. A refusal records the honest reason on the change and audits a
    /// failure; the file is never partially written.
    pub fn apply_proposed_change(
        &mut self,
        run_id: &RunId,
        index: usize,
    ) -> Result<AppliedProposedChange, KernelError> {
        use relux_core::ProposedChangeStatus;
        let (agent_id, task_id, adapter_plugin) = {
            let run = self
                .runs
                .get(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            (
                run.agent_id.clone(),
                run.task_id.clone(),
                run.adapter_plugin.clone(),
            )
        };
        let task_namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());

        // Snapshot the change fields we need (status/action/baseline/path/dest/
        // content) so the borrow of `self.runs` is released before we read other
        // kernel state.
        let (status, action, baseline, rel_path, dest_path, content) = {
            let run = self
                .runs
                .get(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            let change = run.proposed_changes.get(index).ok_or_else(|| {
                KernelError::UnknownProposedChange {
                    run: run_id.to_string(),
                    index,
                }
            })?;
            (
                change.status,
                change.action,
                change.baseline_sha256.clone(),
                change.path.clone(),
                change.dest_path.clone(),
                change.new_content.clone(),
            )
        };

        // 1. Approval is required.
        if status != ProposedChangeStatus::Approved {
            return Err(KernelError::ProposedChangeNotApproved {
                run: run_id.to_string(),
                index,
                status: status.as_str().to_string(),
            });
        }

        // 2. A `replace`/`rename` requires a baseline hash (no force in v1); a
        // `create` has no prior file, so it needs none.
        let baseline = if action.requires_baseline() {
            match baseline {
                Some(b) => Some(b),
                None => {
                    let reason =
                        "no baseline hash recorded; refusing to apply without one (no force in v1)"
                            .to_string();
                    self.refuse_proposed_change(run_id, index, &agent_id, task_namespace.as_ref(), &reason);
                    return Err(KernelError::ProposedChangeNotApplicable {
                        run: run_id.to_string(),
                        index,
                        reason,
                    });
                }
            }
        } else {
            None
        };

        // 3. The run's adapter must have a controlled workspace root.
        let workspace_root = self
            .adapter_runtime_configs
            .get(&adapter_plugin)
            .and_then(|c| c.working_dir.clone())
            .filter(|w| !w.trim().is_empty());
        let workspace_root = match workspace_root {
            Some(w) => w,
            None => {
                let reason = format!(
                    "no controlled workspace root is configured for this run's adapter ({}); \
                     set its working_dir before applying",
                    adapter_plugin.as_str()
                );
                self.refuse_proposed_change(run_id, index, &agent_id, task_namespace.as_ref(), &reason);
                return Err(KernelError::ProposedChangeNotApplicable {
                    run: run_id.to_string(),
                    index,
                    reason,
                });
            }
        };

        // 4-6. Resolve, conflict-check, and write under the controlled root.
        let outcome = apply_change_to_workspace(
            &workspace_root,
            &rel_path,
            action,
            baseline.as_deref(),
            dest_path.as_deref(),
            &content,
        );
        // The path reported as applied is where the file now lives: the destination
        // for a rename, otherwise the change's own path.
        let reported_path = match (action.has_destination(), &dest_path) {
            (true, Some(dest)) => dest.clone(),
            _ => rel_path.clone(),
        };
        match outcome {
            Ok(written_bytes) => {
                let applied_at = self.clock.tick();
                if let Some(run) = self.runs.get_mut(run_id) {
                    if let Some(change) = run.proposed_changes.get_mut(index) {
                        change.status = ProposedChangeStatus::Applied;
                        change.applied_at = Some(applied_at.clone());
                        change.refused_reason = None;
                    }
                }
                let human = match action {
                    relux_core::ProposedChangeAction::Rename => {
                        format!("renamed {rel_path} to {reported_path} ({written_bytes} bytes)")
                    }
                    relux_core::ProposedChangeAction::Delete => {
                        format!("deleted {rel_path} ({written_bytes} bytes)")
                    }
                    _ => {
                        format!("applied proposed change to {reported_path} ({written_bytes} bytes)")
                    }
                };
                self.push_run_event(
                    run_id,
                    "proposed_change_applied",
                    "kernel",
                    &human,
                    serde_json::json!({
                        "index": index,
                        "action": action.as_str(),
                        "path": rel_path,
                        "dest_path": dest_path,
                        "bytes": written_bytes,
                    }),
                );
                self.record_audit(
                    "agent",
                    agent_id.as_str(),
                    "proposed_change:apply",
                    Some("run"),
                    Some(run_id.as_str()),
                    task_namespace.as_ref(),
                    AuditResult::Success,
                    serde_json::json!({
                        "index": index,
                        "action": action.as_str(),
                        "path": rel_path,
                        "dest_path": dest_path,
                        "bytes": written_bytes,
                    }),
                );
                Ok(AppliedProposedChange {
                    run_id: run_id.clone(),
                    index,
                    path: reported_path,
                    bytes: written_bytes,
                    applied_at,
                })
            }
            Err(ApplyFailure { conflict, reason }) => {
                self.refuse_proposed_change(run_id, index, &agent_id, task_namespace.as_ref(), &reason);
                if conflict {
                    Err(KernelError::ProposedChangeConflict {
                        run: run_id.to_string(),
                        index,
                        reason,
                    })
                } else {
                    Err(KernelError::ProposedChangeNotApplicable {
                        run: run_id.to_string(),
                        index,
                        reason,
                    })
                }
            }
        }
    }

    /// Record an honest refusal reason on a proposed change + a failed audit. The
    /// change keeps its `Approved` status (the operator may fix the workspace and
    /// retry) but carries the reason so the dashboard shows why apply was refused.
    fn refuse_proposed_change(
        &mut self,
        run_id: &RunId,
        index: usize,
        agent_id: &AgentId,
        namespace: Option<&NamespaceId>,
        reason: &str,
    ) {
        if let Some(run) = self.runs.get_mut(run_id) {
            if let Some(change) = run.proposed_changes.get_mut(index) {
                change.set_refused_reason(reason);
            }
        }
        self.record_audit(
            "agent",
            agent_id.as_str(),
            "proposed_change:apply",
            Some("run"),
            Some(run_id.as_str()),
            namespace,
            AuditResult::Failed,
            serde_json::json!({ "index": index, "reason": reason }),
        );
    }

    /// Apply a **set** of approved proposed changes for one run as a single
    /// all-or-nothing transaction (master plan section 15 diff/apply model, safety
    /// bar section 17.5). This extends the single-change [`Self::apply_proposed_change`]
    /// with the same strict bar applied to every selected change — but validated
    /// *together first*, so the transaction either writes ALL the files or writes
    /// NONE of them and leaves every status honest.
    ///
    /// The selection is the explicit `indices` of the changes to apply (one run,
    /// so they all share one adapter and therefore one controlled workspace root).
    /// The whole transaction is refused — nothing written — unless EVERY selected
    /// change:
    ///
    /// 1. resolves to a distinct, existing change (no duplicate or unknown index);
    /// 2. is in `Approved` state (approval is still required, per change);
    /// 3. if a `replace`, carries a `baseline_sha256` (v1 refuses without one — no
    ///    force); a `create` carries none;
    /// 4. has a safe relative target path AND a path distinct from every other
    ///    selected change (no two changes may target the same file);
    /// 5. resolves inside the run's configured workspace root with no `..`/symlink
    ///    escape; a `replace` to an existing regular file within the apply cap
    ///    whose current SHA-256 still equals the declared baseline (else a
    ///    **conflict**), a `create` to a path that does NOT yet exist (else a
    ///    **conflict** — never overwritten), with any missing parent directories
    ///    created.
    ///
    /// Only after all of that passes are the files written (temp file + rename per
    /// file). If a write fails mid-apply, the already-written files are rolled back
    /// to the originals captured during validation and an honest failure is
    /// returned — strict up-front validation is preferred precisely so this
    /// rollback path is essentially never reached. On success every applied change
    /// flips to `Applied` (with a shared `applied_at` stamp), one
    /// `proposed_change_set_applied` transcript event + a `proposed_change:apply_set`
    /// audit are recorded, and the per-file results are returned. On any refusal no
    /// file is modified, the honest reason is recorded on each selected change, and
    /// a failed audit is written.
    pub fn apply_proposed_change_set(
        &mut self,
        run_id: &RunId,
        indices: &[usize],
    ) -> Result<AppliedProposedChangeSet, KernelError> {
        use relux_core::ProposedChangeStatus;
        let (agent_id, task_id, adapter_plugin) = {
            let run = self
                .runs
                .get(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            (
                run.agent_id.clone(),
                run.task_id.clone(),
                run.adapter_plugin.clone(),
            )
        };
        let task_namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());

        // 0. The selection must be non-empty with no duplicate indices.
        if indices.is_empty() {
            return Err(KernelError::ProposedChangeSetNotApplicable {
                run: run_id.to_string(),
                reason: "no proposed changes were selected to apply".to_string(),
            });
        }
        let mut seen_idx = std::collections::HashSet::new();
        for &i in indices {
            if !seen_idx.insert(i) {
                return Err(KernelError::ProposedChangeSetNotApplicable {
                    run: run_id.to_string(),
                    reason: format!("change index {i} is listed more than once in the selection"),
                });
            }
        }

        // Snapshot the fields we need for every selected change. An unknown index
        // fails the whole transaction (nothing is written).
        type ChangeSnapshot = (
            usize,
            ProposedChangeStatus,
            relux_core::ProposedChangeAction,
            Option<String>,
            String,
            String,
            Option<String>,
        );
        let snapshot: Vec<ChangeSnapshot> = {
            let run = self
                .runs
                .get(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            let mut out = Vec::with_capacity(indices.len());
            for &i in indices {
                let change = run.proposed_changes.get(i).ok_or_else(|| {
                    KernelError::UnknownProposedChange {
                        run: run_id.to_string(),
                        index: i,
                    }
                })?;
                out.push((
                    i,
                    change.status,
                    change.action,
                    change.baseline_sha256.clone(),
                    change.path.clone(),
                    change.new_content.clone(),
                    change.dest_path.clone(),
                ));
            }
            out
        };

        // 1. Every selected change must be Approved. A not-approved change is an
        // operator precondition (resolve by approving), so — like single apply —
        // we audit the refusal but do NOT stamp a refusal reason on the changes.
        let not_approved: Vec<usize> = snapshot
            .iter()
            .filter(|(_, status, _, _, _, _, _)| *status != ProposedChangeStatus::Approved)
            .map(|(i, _, _, _, _, _, _)| *i)
            .collect();
        if !not_approved.is_empty() {
            let reason = format!(
                "every selected change must be approved before a transactional apply; \
                 not approved: {not_approved:?}"
            );
            self.record_audit(
                "agent",
                agent_id.as_str(),
                "proposed_change:apply_set",
                Some("run"),
                Some(run_id.as_str()),
                task_namespace.as_ref(),
                AuditResult::Failed,
                serde_json::json!({ "indices": indices, "reason": reason }),
            );
            return Err(KernelError::ProposedChangeSetNotApplicable {
                run: run_id.to_string(),
                reason,
            });
        }

        // 2. Every selected `replace`/`rename` must carry a baseline hash (no force
        // in v1). A `create` has no prior file, so it needs none.
        let no_baseline: Vec<usize> = snapshot
            .iter()
            .filter(|(_, _, action, baseline, _, _, _)| {
                action.requires_baseline() && baseline.is_none()
            })
            .map(|(i, _, _, _, _, _, _)| *i)
            .collect();
        if !no_baseline.is_empty() {
            let reason = format!(
                "every replace or rename in a transactional apply must carry a baseline hash \
                 (no force in v1); missing on: {no_baseline:?}"
            );
            self.refuse_proposed_change_set(run_id, indices, &agent_id, task_namespace.as_ref(), &reason);
            return Err(KernelError::ProposedChangeSetNotApplicable {
                run: run_id.to_string(),
                reason,
            });
        }

        // 3. The run's adapter must have one controlled workspace root (a run has a
        // single adapter, so the whole set shares one root by construction).
        let workspace_root = self
            .adapter_runtime_configs
            .get(&adapter_plugin)
            .and_then(|c| c.working_dir.clone())
            .filter(|w| !w.trim().is_empty());
        let workspace_root = match workspace_root {
            Some(w) => w,
            None => {
                let reason = format!(
                    "no controlled workspace root is configured for this run's adapter ({}); \
                     set its working_dir before applying",
                    adapter_plugin.as_str()
                );
                self.refuse_proposed_change_set(run_id, indices, &agent_id, task_namespace.as_ref(), &reason);
                return Err(KernelError::ProposedChangeSetNotApplicable {
                    run: run_id.to_string(),
                    reason,
                });
            }
        };

        // 4-5. Validate every change against the workspace (paths, no duplicate
        // targets, baselines/absence), then write them all transactionally.
        let changes: Vec<PlannedChange> = snapshot
            .iter()
            .map(|(_, _, action, baseline, path, content, dest)| PlannedChange {
                action: *action,
                path: path.clone(),
                dest: dest.clone(),
                baseline: baseline.clone(),
                content: content.clone(),
            })
            .collect();
        match apply_change_set_to_workspace(&workspace_root, &changes) {
            Ok(applied_files) => {
                let applied_at = self.clock.tick();
                let mut applied = Vec::with_capacity(applied_files.len());
                for ((index, _, _, _, _, _, _), (rel_path, written_bytes)) in
                    snapshot.iter().zip(applied_files.iter())
                {
                    if let Some(run) = self.runs.get_mut(run_id) {
                        if let Some(change) = run.proposed_changes.get_mut(*index) {
                            change.status = ProposedChangeStatus::Applied;
                            change.applied_at = Some(applied_at.clone());
                            change.refused_reason = None;
                        }
                    }
                    applied.push(AppliedProposedChange {
                        run_id: run_id.clone(),
                        index: *index,
                        path: rel_path.clone(),
                        bytes: *written_bytes,
                        applied_at: applied_at.clone(),
                    });
                }
                let paths: Vec<&str> = applied.iter().map(|a| a.path.as_str()).collect();
                self.push_run_event(
                    run_id,
                    "proposed_change_set_applied",
                    "kernel",
                    &format!(
                        "applied {} proposed change(s) as one transaction",
                        applied.len()
                    ),
                    serde_json::json!({
                        "indices": indices,
                        "paths": paths,
                        "count": applied.len(),
                    }),
                );
                self.record_audit(
                    "agent",
                    agent_id.as_str(),
                    "proposed_change:apply_set",
                    Some("run"),
                    Some(run_id.as_str()),
                    task_namespace.as_ref(),
                    AuditResult::Success,
                    serde_json::json!({ "indices": indices, "count": applied.len(), "paths": paths }),
                );
                Ok(AppliedProposedChangeSet {
                    run_id: run_id.clone(),
                    applied,
                    applied_at,
                })
            }
            Err(ApplySetFailure { conflict, reason }) => {
                self.refuse_proposed_change_set(run_id, indices, &agent_id, task_namespace.as_ref(), &reason);
                if conflict {
                    Err(KernelError::ProposedChangeSetConflict {
                        run: run_id.to_string(),
                        reason,
                    })
                } else {
                    Err(KernelError::ProposedChangeSetNotApplicable {
                        run: run_id.to_string(),
                        reason,
                    })
                }
            }
        }
    }

    /// Record an honest refusal reason on every selected change in a refused
    /// transactional apply + one failed audit. The changes keep their `Approved`
    /// status (the operator can fix the workspace and retry), but each carries the
    /// reason the transaction was refused so the dashboard can show why.
    fn refuse_proposed_change_set(
        &mut self,
        run_id: &RunId,
        indices: &[usize],
        agent_id: &AgentId,
        namespace: Option<&NamespaceId>,
        reason: &str,
    ) {
        if let Some(run) = self.runs.get_mut(run_id) {
            for &i in indices {
                if let Some(change) = run.proposed_changes.get_mut(i) {
                    change.set_refused_reason(reason);
                }
            }
        }
        self.record_audit(
            "agent",
            agent_id.as_str(),
            "proposed_change:apply_set",
            Some("run"),
            Some(run_id.as_str()),
            namespace,
            AuditResult::Failed,
            serde_json::json!({ "indices": indices, "reason": reason }),
        );
    }

    /// Retry a failed run as a **fresh** run on the same task
    /// (`docs/RELUX_MASTER_PLAN.md` section 10.2 `prime.retry_run`). This is an
    /// honest re-attempt, not a resume of a partial CLI run (resuming a partial
    /// run is explicitly out of scope for Adapter Runtime v1): the task is
    /// re-queued and dispatched through its assigned agent's adapter again, with
    /// the same safety gating (enabled runtime, binary on PATH, permission check)
    /// as the first attempt. The new run records its lineage via `retried_from`.
    ///
    /// Refuses honestly when the run is not in a retryable (`Failed`) state, when
    /// the task no longer exists, or when it has no assigned agent.
    pub fn retry_run(&mut self, run_id: &RunId) -> Result<RunId, KernelError> {
        let (task_id, status) = {
            let run = self
                .runs
                .get(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            (run.task_id.clone(), run.status.clone())
        };
        if status != RunStatus::Failed {
            return Err(KernelError::RunNotRetryable {
                run: run_id.to_string(),
                status: format!("{status:?}"),
            });
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
        let namespace = task.namespace_id.clone();
        if task.assigned_agent.is_none() {
            return Err(KernelError::TaskNotAssigned(task_id.to_string()));
        }

        // Re-queue the task so a fresh run can start, then dispatch it through the
        // same assigned-adapter path used by the original attempt. We capture the
        // outcome rather than `?`-propagating it so the retry's lineage is stamped
        // even when this attempt also fails (an honest, recorded re-attempt).
        if let Some(task) = self.tasks.get_mut(&task_id) {
            task.status = TaskStatus::Queued;
            task.updated_at = self.clock.tick();
        }
        let result = self.execute_assigned_run(&task_id);

        // The fresh run is the highest-numbered run (ids are monotonic and
        // zero-padded, so lexicographic max == newest). It exists whether the
        // attempt succeeded or failed honestly.
        let new_run_id = self
            .runs
            .values()
            .max_by_key(|r| r.id.0.clone())
            .map(|r| r.id.clone())
            .filter(|id| id != run_id);

        if let Some(new_run_id) = new_run_id.clone() {
            if let Some(run) = self.runs.get_mut(&new_run_id) {
                run.retried_from = Some(run_id.clone());
            }
            self.push_run_event(
                &new_run_id,
                "run_retried_from",
                "kernel",
                &format!("retry of failed run {run_id}"),
                serde_json::json!({ "retried_from": run_id.as_str() }),
            );
            self.record_audit(
                "user",
                "founder",
                "run:retry",
                Some("run"),
                Some(new_run_id.as_str()),
                Some(&namespace),
                if result.is_ok() {
                    AuditResult::Success
                } else {
                    AuditResult::Failed
                },
                serde_json::json!({ "retried_from": run_id.as_str(), "task": task_id.as_str() }),
            );
        }

        result
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
            note: None,
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
        note: Option<String>,
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
            approval.note = note;
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
            all_agent_ids: self.agents.keys().map(|id| id.0.clone()).collect(),
            all_task_ids: self.tasks.keys().map(|id| id.0.clone()).collect(),
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

        let mut turn = match plan {
            PrimePlan::Reply { text } => PrimeTurn {
                intent,
                reply: text,
                disposition: PrimeDisposition::Answered,
                action: None,
                created_task: None,
                started_run: None,
                created_agent: None,
                approval: None,
                invoked_tool: None,
                tool_output: None,
                tool_error: None,
                suggested_actions: Vec::new(),
            },
            PrimePlan::Clarify { text } => PrimeTurn {
                intent,
                reply: text,
                disposition: PrimeDisposition::NeedsClarification,
                action: None,
                created_task: None,
                started_run: None,
                created_agent: None,
                approval: None,
                invoked_tool: None,
                tool_output: None,
                tool_error: None,
                suggested_actions: Vec::new(),
            },
            PrimePlan::Act { action, text } => self.prime_execute(ctx, intent, action, text)?,
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
                PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::AwaitingApproval,
                    action: Some(action),
                    created_task: None,
                    started_run: None,
                    created_agent: None,
                    approval: Some(approval),
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
                }
            }
        };
        // One central place to offer the next-step buttons the chat surface
        // renders (`docs/RELUX_MASTER_PLAN.md` §11.1 "Prime suggested next
        // actions"). Each is just a pre-written user message, so it can do
        // nothing the user could not type.
        attach_suggestions(&mut turn, message, &summary);
        Ok(turn)
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
                    "{text} Created {task} and assigned it to {}. It is ready to run whenever you are.",
                    ctx.agent
                );
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: Some(task),
                    started_run: None,
                    created_agent: None,
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
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

                let reply = format!(
                    "{text} Created {task} and started {run}. The task is now running and awaiting further action from the assigned agent."
                );
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: Some(task),
                    started_run: Some(run),
                    created_agent: None,
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
                })
            }
            PrimeAction::StartRun { task_id } => {
                let tid = TaskId::new(task_id.clone());
                let run = self.start_run(&tid)?;

                let reply = format!(
                    "{text} Started {run}. The task is now running and awaiting further action from the assigned agent."
                );
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: None,
                    started_run: Some(run),
                    created_agent: None,
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
                })
            }
            PrimeAction::CreateAgent {
                name,
                adapter_plugin,
            } => {
                let agent_id_str = name.to_lowercase().replace(" ", "-");
                let adapter = PluginId::new(adapter_plugin.clone());
                let agent_id = self.create_agent(
                    &agent_id_str,
                    name,
                    "Agent created by Prime", // Default description
                    &adapter,
                    &ctx.namespace,
                    None,   // No persona
                    vec![], // No special permissions by default
                )?;
                let reply = format!("{text} Created agent {agent_id}.");
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: None,
                    started_run: None,
                    created_agent: Some(agent_id),
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
                })
            }
            PrimeAction::AssignTask { task_id, agent_id } => {
                let task_obj_id = TaskId::new(task_id.clone());
                let agent_obj_id = AgentId::new(agent_id.clone());
                self.assign_task(&task_obj_id, &agent_obj_id)?;
                let reply = format!("{text} Assigned task {} to agent {}.", task_id, agent_id);
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Executed,
                    action: Some(action),
                    created_task: None,
                    started_run: None,
                    created_agent: None,
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
                })
            }
            PrimeAction::DiscoverTools => {
                // Hide internal dev/test fixtures (echo) from the user-facing
                // catalogue so Prime never offers it as a real ability.
                let tools: Vec<_> = self
                    .discover_tools(Some(&ctx.agent))
                    .into_iter()
                    .filter(|t| !crate::builtin::is_internal_plugin(&t.plugin_id))
                    .collect();
                let reply = render_tool_catalog(&text, &tools);
                Ok(PrimeTurn {
                    intent,
                    reply,
                    disposition: PrimeDisposition::Answered,
                    action: Some(action),
                    created_task: None,
                    started_run: None,
                    created_agent: None,
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
                })
            }
            PrimeAction::InvokeTool {
                plugin_id,
                tool_name,
                input_json,
            } => self.prime_invoke_tool(ctx, intent, &action, &text, plugin_id, tool_name, input_json),
            PrimeAction::OrchestrateGoal { goal } => match self.prime_orchestrate(ctx, goal) {
                Ok(record) => {
                    let reply = format!("{text} {}", render_orchestration_plan(&record));
                    Ok(PrimeTurn {
                        intent,
                        reply,
                        disposition: PrimeDisposition::Executed,
                        // Surface the first brief as the "created task" so existing
                        // UI/clients that follow `created_task` still navigate
                        // somewhere sensible; the full chain is in the orchestration.
                        created_task: record.steps.first().map(|s| s.task_id.clone()),
                        action: Some(action),
                        started_run: None,
                        created_agent: None,
                        approval: None,
                        invoked_tool: None,
                        tool_output: None,
                        tool_error: None,
                        suggested_actions: Vec::new(),
                    })
                }
                Err(KernelError::OrchestrationNotMultiAgent) => Ok(PrimeTurn {
                    intent,
                    reply: "That goal reads as a single piece of work, not something to split across agents. Tell me the distinct steps, or ask me to create one task.".to_string(),
                    disposition: PrimeDisposition::NeedsClarification,
                    action: Some(action),
                    created_task: None,
                    started_run: None,
                    created_agent: None,
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
                    suggested_actions: Vec::new(),
                }),
                Err(e) => Err(e),
            },
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
                created_agent: None,
                approval: None,
                invoked_tool: None,
                tool_output: None,
                tool_error: None,
                suggested_actions: Vec::new(),
            }),
        }
    }

    /// Run one tool for a Prime `ToolInvocation`/`StatusQuestion` turn, honestly.
    ///
    /// Resolution is grounded in [`Self::discover_tools`]: an empty `tool_name`
    /// resolves to the named plugin's first installed tool, and the tool's
    /// executable status decides the outcome. Only a `Ready` tool is actually
    /// invoked (through [`Self::invoke_tool`], the same permission/audit path as
    /// `/v1/relux/tools/invoke`). A `not_implemented` or `missing_permission`
    /// tool - or an unknown one - is reported with a clear `tool_error` and NO
    /// fabricated output, never erroring the whole turn
    /// (`docs/RELUX_MASTER_PLAN.md` §11.1, "Tool Invocation Surface").
    #[allow(clippy::too_many_arguments)]
    fn prime_invoke_tool(
        &mut self,
        ctx: &PrimeContext,
        intent: relux_core::PrimeIntent,
        action: &PrimeAction,
        text: &str,
        plugin_id: &str,
        tool_name: &str,
        input_json: &str,
    ) -> Result<PrimeTurn, KernelError> {
        // Prose answers (status) ride along on `text`; keep it as the fallback so
        // a status reply stays readable even if the tool cannot run.
        let prose = if plugin_id == "relux-tools-status" {
            text.trim().to_string()
        } else {
            String::new()
        };
        let with_prose = |note: String| -> String {
            if prose.is_empty() {
                note
            } else {
                format!("{prose} ({note})")
            }
        };
        let turn = |reply: String,
                    disposition: PrimeDisposition,
                    invoked_tool: Option<String>,
                    tool_output: Option<serde_json::Value>,
                    tool_error: Option<String>|
         -> PrimeTurn {
            PrimeTurn {
                intent: intent.clone(),
                reply,
                disposition,
                action: Some(action.clone()),
                created_task: None,
                started_run: None,
                created_agent: None,
                approval: None,
                invoked_tool,
                tool_output,
                tool_error,
                suggested_actions: Vec::new(),
            }
        };

        let descriptors = self.discover_tools(Some(&ctx.agent));
        // Resolve an empty tool name to the plugin's first installed tool.
        let resolved_tool = if tool_name.is_empty() {
            descriptors
                .iter()
                .find(|d| d.plugin_id == plugin_id)
                .map(|d| d.tool_name.clone())
        } else {
            Some(tool_name.to_string())
        };
        let Some(tool) = resolved_tool else {
            let note = format!(
                "no installed tool named {plugin_id}; ask \"what tools can you use?\" to see what is available"
            );
            return Ok(turn(
                with_prose(note.clone()),
                PrimeDisposition::NeedsClarification,
                None,
                None,
                Some(note),
            ));
        };
        let label = format!("{plugin_id}/{tool}");

        let descriptor = descriptors
            .iter()
            .find(|d| d.plugin_id == plugin_id && d.tool_name == tool);
        match descriptor.map(|d| d.executable.clone()) {
            None => {
                let note = format!("I could not find {label} among the installed tools");
                Ok(turn(
                    with_prose(note.clone()),
                    PrimeDisposition::NeedsClarification,
                    None,
                    None,
                    Some(note),
                ))
            }
            Some(ToolExecutability::NotImplemented) => {
                let note = format!(
                    "{label} is installed and discoverable, but this local runtime cannot execute it yet"
                );
                Ok(turn(
                    with_prose(note.clone()),
                    PrimeDisposition::NeedsClarification,
                    None,
                    None,
                    Some(note),
                ))
            }
            Some(ToolExecutability::RuntimeNotConfigured) => {
                let note = format!(
                    "{label} is installed but has no runtime configured; an operator must set an HTTP loopback endpoint for {plugin_id} before it can run"
                );
                Ok(turn(
                    with_prose(note.clone()),
                    PrimeDisposition::NeedsClarification,
                    None,
                    None,
                    Some(note),
                ))
            }
            Some(ToolExecutability::RuntimeDisabled) => {
                let note = format!(
                    "{label} has an HTTP loopback runtime configured but it is disabled; re-enable it first"
                );
                Ok(turn(
                    with_prose(note.clone()),
                    PrimeDisposition::NeedsClarification,
                    None,
                    None,
                    Some(note),
                ))
            }
            Some(ToolExecutability::MissingPermission) => {
                let note = format!(
                    "I cannot run {label}: I do not hold the permission it requires - grant it first, then ask me again"
                );
                Ok(turn(
                    with_prose(note.clone()),
                    PrimeDisposition::NeedsClarification,
                    None,
                    None,
                    Some(note),
                ))
            }
            Some(ToolExecutability::Ready) => {
                let input: serde_json::Value =
                    serde_json::from_str(input_json).unwrap_or_else(|_| serde_json::json!({}));
                match self.invoke_tool(
                    &ctx.agent,
                    &PluginId::new(plugin_id.to_string()),
                    &tool,
                    input,
                ) {
                    Ok(result) => {
                        // The reply stays concise ("Running <tool>." / the status
                        // prose); the real JSON rides in `tool_output`, so it is
                        // never restated in prose.
                        let reply = if prose.is_empty() {
                            text.trim().to_string()
                        } else {
                            prose.clone()
                        };
                        Ok(turn(
                            reply.trim().to_string(),
                            PrimeDisposition::Executed,
                            Some(label),
                            Some(result.output),
                            None,
                        ))
                    }
                    Err(e) => {
                        let note = format!("{label} could not run: {e}");
                        Ok(turn(
                            with_prose(note.clone()),
                            PrimeDisposition::NeedsClarification,
                            None,
                            None,
                            Some(note),
                        ))
                    }
                }
            }
        }
    }

    /// Executes a running task locally using the echo tool and completes it.
    /// This is a temporary deterministic local execution path for MVP.
    pub fn execute_local_run(&mut self, task_id: &TaskId) -> Result<RunId, KernelError> {
        let task = self.task(task_id).ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
        let assigned_agent_id = task.assigned_agent.clone().ok_or_else(|| KernelError::TaskNotAssigned(task_id.to_string()))?;

        // Find the most recent run for this task that is currently running
        let run_id = self.runs.values()
            .filter(|r| r.task_id == *task_id && r.status == RunStatus::Running)
            .max_by_key(|r| r.started_at.clone())
            .map(|r| r.id.clone())
            .ok_or_else(|| KernelError::NoActiveRun(task_id.to_string()))?;

        let agent = self.agent(&assigned_agent_id).ok_or_else(|| KernelError::UnknownAgent(assigned_agent_id.to_string()))?;
        let agent_id_for_call = agent.id.clone();

        // Perform the echo cycle on the task's own input.
        let echo_plugin = PluginId::new("relux-tools-echo");
        let input = self.task(task_id)
            .map(|t| t.input.clone())
            .unwrap_or_else(|| serde_json::json!({})); // Fallback to empty JSON if no input

        self.call_tool(&run_id, &agent_id_for_call, &echo_plugin, "echo.say", input)?;
        self.complete_run(&run_id, "echo.say returned the input unchanged")?;
        self.complete_task(task_id)?;

        Ok(run_id)
    }

    /// Execute an assigned task through its agent's adapter, dispatching on the
    /// adapter kind (`docs/RELUX_MASTER_PLAN.md` section 8.1, Adapter Runtime v1):
    ///
    /// - the local Prime adapter runs the deterministic echo path;
    /// - a CLI adapter (Claude/Codex/generic command) with an **enabled** runtime
    ///   spawns that local CLI in a bounded, non-interactive, non-bypass mode;
    /// - anything else fails honestly (disabled, not configured, binary missing,
    ///   timeout, or non-zero exit) - never a fabricated success.
    ///
    /// This is the entry point behind the Work page's "Run (Assigned)" action and
    /// the `task run-assigned` CLI. It starts a run first if the task is still
    /// `Created`/`Queued` (permission-checked in [`start_run`]).
    pub fn execute_assigned_run(&mut self, task_id: &TaskId) -> Result<RunId, KernelError> {
        let (agent_id, status) = {
            let task = self
                .tasks
                .get(task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            let agent_id = task
                .assigned_agent
                .clone()
                .ok_or_else(|| KernelError::TaskNotAssigned(task_id.to_string()))?;
            (agent_id, task.status.clone())
        };
        let adapter = self
            .agents
            .get(&agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?
            .adapter_plugin
            .clone();

        if matches!(status, TaskStatus::Created | TaskStatus::Queued) {
            self.start_run(task_id)?;
        }

        match recognize_adapter_kind(adapter.as_str()) {
            Some(AdapterKind::LocalPrime) => self.execute_local_run(task_id),
            // Every other adapter (recognized CLI or generic) runs through the CLI
            // path, which requires an explicitly enabled runtime.
            _ => self.execute_cli_run(task_id, &adapter),
        }
    }

    /// Execute the assigned task by spawning the adapter's local CLI. Gated on an
    /// enabled runtime + a binary on PATH; every outcome is recorded on the run
    /// transcript and audit log. On failure the run and task are marked failed.
    ///
    /// This is the sequential, single-lock path: it prepares the spawn, runs the
    /// process inline, and finalizes the result in one call. The parallel
    /// orchestration driver uses the same two halves ([`prepare_cli_run`] +
    /// [`finalize_cli_run`]) but spawns the process OUTSIDE the kernel lock so
    /// several briefs' CLIs run on real OS threads at once.
    fn execute_cli_run(
        &mut self,
        task_id: &TaskId,
        adapter: &PluginId,
    ) -> Result<RunId, KernelError> {
        let plan = self.prepare_cli_run(task_id, adapter)?;
        // The one place the kernel touches a real CLI. In the sequential path this
        // runs inline; the parallel path calls the same function on a worker thread
        // with the kernel lock released (see [`PreparedBrief::run`]).
        let outcome = crate::adapter::run_adapter_command(&plan.spec);
        self.finalize_cli_run(plan, outcome)
    }

    /// Prepare a CLI adapter spawn under the kernel lock: require an enabled
    /// runtime, resolve the binary on PATH, compose the prompt, build the redaction-
    /// ready [`AdapterCommandSpec`], and record the `adapter_spawn` transcript
    /// event. Returns a [`CliExecPlan`] whose `spec` carries no `&self`, so the
    /// actual blocking process spawn can happen with the lock released. Every early
    /// exit (disabled/unconfigured runtime, missing binary, invalid config) marks
    /// the run + task failed and returns the same honest error as before — never a
    /// fabricated success, and never an auto-run of downloaded plugin code (only an
    /// explicitly enabled, operator-configured local binary is ever spawned).
    fn prepare_cli_run(
        &mut self,
        task_id: &TaskId,
        adapter: &PluginId,
    ) -> Result<CliExecPlan, KernelError> {
        let run_id = self
            .runs
            .values()
            .filter(|r| r.task_id == *task_id && r.status == RunStatus::Running)
            .max_by_key(|r| r.started_at.clone())
            .map(|r| r.id.clone())
            .ok_or_else(|| KernelError::NoActiveRun(task_id.to_string()))?;
        let namespace = self.tasks.get(task_id).map(|t| t.namespace_id.clone());

        // 1. Require an enabled runtime (CLI adapters are disabled by default).
        let config = match self.adapter_runtime_configs.get(adapter).cloned() {
            Some(c) if c.enabled => c,
            Some(_) => {
                let err = KernelError::AdapterRuntimeDisabled {
                    plugin: adapter.to_string(),
                };
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string());
                return Err(err);
            }
            None => {
                let err = KernelError::AdapterRuntimeNotConfigured {
                    plugin: adapter.to_string(),
                };
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string());
                return Err(err);
            }
        };

        // 2. Resolve the binary and confirm it is on PATH.
        let binary = match config.resolved_command() {
            Some(b) => b,
            None => {
                let err = KernelError::InvalidAdapterConfig {
                    plugin: adapter.to_string(),
                    message: "no command configured".to_string(),
                };
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string());
                return Err(err);
            }
        };
        // Resolve to the actual on-disk path. On Windows this turns a bare
        // `claude` into the full path of `claude.cmd`/`claude.exe`, which the
        // process spawner can run (a bare extension-less shim cannot be spawned).
        let program = match crate::adapter::find_on_path(&binary) {
            Some(p) => p.to_string_lossy().to_string(),
            None => {
                let err = KernelError::AdapterBinaryMissing {
                    plugin: adapter.to_string(),
                    binary: binary.clone(),
                };
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string());
                return Err(err);
            }
        };

        // 3. Compose the prompt from the agent persona + task title/input.
        let (agent_name, persona) = {
            let agent = self
                .runs
                .get(&run_id)
                .and_then(|r| self.agents.get(&r.agent_id))
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            (agent.name.clone(), agent.persona.clone())
        };
        let (task_title, task_input) = {
            let task = self
                .tasks
                .get(task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            (task.title.clone(), task.input.clone())
        };
        let prompt = crate::adapter::compose_prompt(
            &agent_name,
            persona.as_deref(),
            &task_title,
            &task_input,
        );
        let args = crate::adapter::build_adapter_args(&config.kind);
        let spec = crate::adapter::AdapterCommandSpec {
            program: program.clone(),
            args,
            stdin: prompt,
            working_dir: config.working_dir.clone(),
            timeout: std::time::Duration::from_secs(config.timeout_seconds),
            max_output_bytes: config.max_output_bytes as usize,
        };

        self.push_run_event(
            &run_id,
            "adapter_spawn",
            "kernel",
            &format!(
                "spawning {} adapter '{}' ({} arg(s))",
                config.kind.as_str(),
                binary,
                spec.args.len()
            ),
            serde_json::json!({
                "adapter": adapter.as_str(),
                "kind": config.kind.as_str(),
                "program": binary,
                "arg_count": spec.args.len(),
            }),
        );

        Ok(CliExecPlan {
            run_id,
            task_id: task_id.clone(),
            adapter: adapter.clone(),
            kind: config.kind,
            binary,
            namespace,
            timeout_seconds: config.timeout_seconds,
            spec,
        })
    }

    /// Finalize a CLI adapter run under the kernel lock from the process `outcome`
    /// produced by [`crate::adapter::run_adapter_command`] (run inline in the
    /// sequential path, or on a worker thread in the parallel path). Parses any
    /// structured envelope, records the (already-redacted) output on the transcript,
    /// completes or fails the run + task, writes metrics + audit, and returns the
    /// run id on success. Behaviour is identical to the old inline body — only the
    /// process spawn was lifted out so it can run without the lock.
    fn finalize_cli_run(
        &mut self,
        plan: CliExecPlan,
        outcome: std::io::Result<crate::adapter::AdapterRunOutcome>,
    ) -> Result<RunId, KernelError> {
        let CliExecPlan {
            run_id,
            task_id,
            adapter,
            kind: config_kind,
            binary,
            namespace,
            timeout_seconds,
            spec: _,
        } = plan;
        let task_id = &task_id;
        let adapter = &adapter;
        match outcome {
            Ok(outcome) if outcome.success => {
                // Parse a structured result envelope when the CLI emitted one
                // (Claude `--output-format json`); otherwise surface plain text.
                // Never fabricate a tool call or success.
                let parsed = relux_core::parse_adapter_result(&outcome.stdout, config_kind);
                let summary = render_adapter_summary(&binary, &outcome, &parsed);
                self.push_run_event(
                    &run_id,
                    "adapter_output",
                    "adapter",
                    &summary,
                    serde_json::json!({
                        "exit_code": outcome.exit_code,
                        "stdout": outcome.stdout,
                        "stderr": outcome.stderr,
                        "stdout_truncated": outcome.stdout_truncated,
                        "stderr_truncated": outcome.stderr_truncated,
                        "duration_ms": outcome.duration_ms,
                        "structured": parsed.structured,
                        "is_error": parsed.is_error,
                        "cost_usd": parsed.cost_usd,
                        "num_turns": parsed.num_turns,
                        "artifacts": parsed.artifacts.len(),
                        "proposed_changes": parsed.proposed_changes.len(),
                    }),
                );

                // An exit code of 0 is not always success: a structured envelope
                // can report `is_error: true` (e.g. a rate limit). Honour that so
                // we never record a fabricated success.
                if parsed.is_error == Some(true) {
                    let reason = format!(
                        "adapter '{}' reported an error: {}",
                        binary,
                        first_line(&parsed.text)
                    );
                    self.set_run_metrics(
                        &run_id,
                        outcome.duration_ms,
                        parsed.usage.clone(),
                        parsed.cost_usd,
                    );
                    // An errored envelope may still reference artifacts it
                    // produced before failing - capture them read-only, plus any
                    // proposed changes (captured for review; apply still requires
                    // an explicit approval + action and is never automatic).
                    self.set_run_artifacts(&run_id, parsed.artifacts.clone());
                    self.set_run_proposed_changes(&run_id, parsed.proposed_changes.clone());
                    self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &reason);
                    return Err(KernelError::AdapterExecutionFailed {
                        plugin: adapter.to_string(),
                        message: reason,
                    });
                }

                self.complete_run(&run_id, &summary)?;
                self.set_run_metrics(
                    &run_id,
                    outcome.duration_ms,
                    parsed.usage.clone(),
                    parsed.cost_usd,
                );
                self.set_run_artifacts(&run_id, parsed.artifacts.clone());
                self.set_run_proposed_changes(&run_id, parsed.proposed_changes.clone());
                self.complete_task(task_id)?;
                let agent = self
                    .runs
                    .get(&run_id)
                    .map(|r| r.agent_id.as_str().to_string())
                    .unwrap_or_else(|| "agent".to_string());
                self.record_audit(
                    "agent",
                    &agent,
                    "adapter:execute",
                    Some("adapter"),
                    Some(adapter.as_str()),
                    namespace.as_ref(),
                    AuditResult::Success,
                    serde_json::json!({ "run": run_id.as_str(), "exit_code": outcome.exit_code }),
                );
                Ok(run_id)
            }
            Ok(outcome) => {
                let reason = if outcome.timed_out {
                    format!(
                        "adapter '{}' timed out after {}s",
                        binary, timeout_seconds
                    )
                } else {
                    format!(
                        "adapter '{}' exited with code {}",
                        binary,
                        outcome
                            .exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    )
                };
                self.push_run_event(
                    &run_id,
                    "adapter_output",
                    "adapter",
                    &reason,
                    serde_json::json!({
                        "exit_code": outcome.exit_code,
                        "timed_out": outcome.timed_out,
                        "stdout": outcome.stdout,
                        "stderr": outcome.stderr,
                        "duration_ms": outcome.duration_ms,
                    }),
                );
                self.set_run_metrics(&run_id, outcome.duration_ms, None, None);
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &reason);
                Err(KernelError::AdapterExecutionFailed {
                    plugin: adapter.to_string(),
                    message: reason,
                })
            }
            Err(e) => {
                let reason = format!("failed to spawn adapter '{binary}': {e}");
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &reason);
                Err(KernelError::AdapterExecutionFailed {
                    plugin: adapter.to_string(),
                    message: reason,
                })
            }
        }
    }

    /// Mark a CLI run + its task failed and audit the failure. Shared by every
    /// honest failure exit of [`execute_cli_run`].
    fn fail_cli_run(
        &mut self,
        run_id: &RunId,
        task_id: &TaskId,
        namespace: Option<&NamespaceId>,
        adapter: &PluginId,
        reason: &str,
    ) {
        let agent = self
            .runs
            .get(run_id)
            .map(|r| r.agent_id.as_str().to_string())
            .unwrap_or_else(|| "agent".to_string());
        let _ = self.fail_run(run_id, reason);
        let _ = self.fail_task(task_id);
        self.record_audit(
            "agent",
            &agent,
            "adapter:execute",
            Some("adapter"),
            Some(adapter.as_str()),
            namespace,
            AuditResult::Failed,
            serde_json::json!({ "run": run_id.as_str(), "reason": reason }),
        );
    }

    /// Mark a run failed with an error message and record it on the transcript.
    pub fn fail_run(&mut self, run_id: &RunId, error: &str) -> Result<(), KernelError> {
        let ended = self.clock.tick();
        let (agent_id, task_id) = {
            let run = self
                .runs
                .get_mut(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            run.status = RunStatus::Failed;
            run.ended_at = Some(ended);
            run.error = Some(error.to_string());
            (run.agent_id.clone(), run.task_id.clone())
        };
        let task_namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());
        self.push_run_event(run_id, "run_failed", "kernel", error, serde_json::Value::Null);
        self.record_audit(
            "agent",
            agent_id.as_str(),
            "run:fail",
            Some("run"),
            Some(run_id.as_str()),
            task_namespace.as_ref(),
            AuditResult::Failed,
            serde_json::Value::Null,
        );
        Ok(())
    }

    /// Mark a task failed.
    pub fn fail_task(&mut self, task_id: &TaskId) -> Result<(), KernelError> {
        let now = self.clock.tick();
        let namespace = {
            let task = self
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            task.status = TaskStatus::Failed;
            task.updated_at = now;
            task.namespace_id.clone()
        };
        self.record_audit(
            "kernel",
            "kernel",
            "task:fail",
            Some("task"),
            Some(task_id.as_str()),
            Some(&namespace),
            AuditResult::Failed,
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

    /// All runs, sorted by id for deterministic listing.
    pub fn runs(&self) -> Vec<&Run> {
        let mut out: Vec<&Run> = self.runs.values().collect();
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        out
    }

    /// The transcript for one run, in emission order.
    pub fn run_events(&self, run_id: &RunId) -> Vec<&RunEvent> {
        self.run_events_since(run_id, None)
    }

    /// The transcript for one run in emission order, optionally only the tail
    /// strictly AFTER `since` (an exclusive event-id cursor). This backs the
    /// dashboard's incremental live-tail for the Relux run model: a panel left
    /// open during a long run re-fetches only the new events past its cursor
    /// instead of the whole transcript each poll.
    ///
    /// A `None`, empty, or unparseable cursor returns the FULL transcript, so a
    /// client that lost its place (or sent `since=0`) degrades to a full fetch
    /// rather than silently dropping events.
    pub fn run_events_since(&self, run_id: &RunId, since: Option<&str>) -> Vec<&RunEvent> {
        let cursor = since.and_then(run_event_seq);
        self.run_events
            .iter()
            .filter(|e| &e.run_id == run_id)
            .filter(|e| match cursor {
                // Keep only events whose sequence is strictly past the cursor.
                // An id we can't parse is kept (never drop a real event).
                Some(c) => run_event_seq(&e.id).map(|n| n > c).unwrap_or(true),
                None => true,
            })
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

/// Parse the numeric sequence from a `revent_NNNN` event id. The kernel mints
/// ids as `revent_{:04}` off a monotonic counter, so the numeric suffix orders
/// events even past the 4-digit zero-pad width (where lexicographic compare on
/// the raw id would break). Returns `None` for an id with no parseable suffix,
/// so the caller treats it as "keep" rather than guessing an order.
fn run_event_seq(id: &str) -> Option<u64> {
    id.rsplit('_').next().and_then(|n| n.parse::<u64>().ok())
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

/// A fully-resolved CLI adapter invocation, produced by `prepare_cli_run` under
/// the kernel lock and consumed by `finalize_cli_run`. Everything the spawn needs
/// is captured here so the blocking `run_adapter_command` call can run with the
/// lock released. Plain data (no `&self`), so it is `Send` and crosses to a worker
/// thread for true OS-parallel execution.
struct CliExecPlan {
    run_id: RunId,
    task_id: TaskId,
    adapter: PluginId,
    kind: AdapterKind,
    binary: String,
    namespace: Option<NamespaceId>,
    timeout_seconds: u64,
    spec: crate::adapter::AdapterCommandSpec,
}

/// One ready brief whose adapter spawn has been prepared under the kernel lock but
/// not yet run. The orchestration driver collects up to the concurrency cap of
/// these, releases the lock, runs them on real OS threads via [`Self::run`], then
/// merges the results back under the lock. `Send` (only plain data), so it moves
/// freely across threads.
pub struct PreparedBrief {
    step_index: usize,
    round_no: u32,
    agent_label: String,
    started_at: String,
    plan: CliExecPlan,
}

impl PreparedBrief {
    /// The task this prepared brief will run (for the in-flight poll view).
    pub fn task_id(&self) -> &TaskId {
        &self.plan.task_id
    }

    /// Run the prepared adapter process. NO kernel access — pure blocking I/O on
    /// the already-resolved, redaction-ready spec, safe to call on a worker thread
    /// while the kernel lock is released. The returned [`FinishedBrief`] is merged
    /// back into the orchestration record under the lock by
    /// [`KernelState::finalize_prepared_brief`].
    pub fn run(self) -> FinishedBrief {
        let outcome = crate::adapter::run_adapter_command(&self.plan.spec);
        FinishedBrief {
            step_index: self.step_index,
            round_no: self.round_no,
            agent_label: self.agent_label,
            started_at: self.started_at,
            plan: self.plan,
            outcome,
        }
    }
}

/// A prepared brief whose adapter process has finished running (off-lock). Carries
/// the raw process outcome plus the identifiers needed to finalize it back under
/// the kernel lock. `Send` (the outcome is plain data + an `io::Error`).
pub struct FinishedBrief {
    step_index: usize,
    round_no: u32,
    agent_label: String,
    started_at: String,
    plan: CliExecPlan,
    outcome: std::io::Result<crate::adapter::AdapterRunOutcome>,
}

/// The outcome of preparing one dependency-aware round under the lock: how many
/// briefs were resolved inline (local-echo briefs and briefs blocked/failed before
/// any spawn) and the CLI briefs still to run off-lock. A round "ran" when
/// `ran_inline + prepared.len() > 0`; an empty result means nothing was ready.
pub struct RoundPrep {
    /// Briefs fully resolved under the lock this round (local echo, or a runtime/
    /// binary block recorded before any process spawn).
    pub ran_inline: u32,
    /// CLI briefs prepared for off-lock parallel execution.
    pub prepared: Vec<PreparedBrief>,
}

impl RoundPrep {
    /// True when at least one brief was attempted this round (inline or prepared).
    pub fn ran(&self) -> bool {
        self.ran_inline > 0 || !self.prepared.is_empty()
    }
}

/// Run every prepared brief's adapter process concurrently on its own OS thread,
/// joining all before returning. The set is already bounded to the concurrency cap
/// (<= 4) by the round scheduler, so this spawns at most that many threads. A single
/// brief is run inline (no thread). A brief whose thread panics is dropped from the
/// result — its step stays pending (it was stamped, not finalized) and re-runs next
/// round — so one brief can never take the others down.
///
/// This is the ONE off-lock spawn primitive shared by both orchestration drivers:
/// the synchronous in-kernel driver ([`KernelState::run_orchestration`], used by the
/// blocking `/run` API and the `prime orchestration run` CLI) calls it directly
/// between its prepare and finalize phases; the HTTP server's non-blocking job
/// driver calls it with the kernel lock released. Both therefore get identical true
/// bounded OS-parallel adapter execution. Results come back in the input order (the
/// joins block in order), so the caller's finalize sequence stays deterministic.
pub fn run_briefs_in_parallel(prepared: Vec<PreparedBrief>) -> Vec<FinishedBrief> {
    if prepared.len() <= 1 {
        return prepared.into_iter().map(|p| p.run()).collect();
    }
    let handles: Vec<_> = prepared
        .into_iter()
        .map(|p| std::thread::spawn(move || p.run()))
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

/// Render a concise, already-redacted run summary from an adapter outcome. Uses
/// the parsed result text (the envelope's `result` when the CLI emitted one, else
/// the raw stdout) so the summary is the human-meaningful line, not a wall of
/// JSON. The snippet is bounded so a long transcript never bloats the summary.
fn render_adapter_summary(
    binary: &str,
    outcome: &crate::adapter::AdapterRunOutcome,
    parsed: &relux_core::AdapterResultSummary,
) -> String {
    let mut s = format!("adapter '{binary}' completed (exit 0)");
    let text = parsed.text.trim();
    if !text.is_empty() {
        let snippet: String = text.chars().take(280).collect();
        s.push_str(": ");
        s.push_str(&snippet);
        if outcome.stdout_truncated || text.chars().count() > 280 {
            s.push_str(" …");
        }
    }
    s
}

/// The first non-empty line of a block of text, bounded, for one-line summaries.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// The result of a successful proposed-change apply, returned to the API layer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AppliedProposedChange {
    pub run_id: RunId,
    pub index: usize,
    /// The safe relative path that was written.
    pub path: String,
    /// The number of bytes written.
    pub bytes: u64,
    /// The logical-clock stamp recorded at apply time.
    pub applied_at: String,
}

/// The result of a successful **transactional** proposed-change apply (master
/// plan section 15). Either every selected change was applied or none were, so
/// `applied` lists exactly the changes that were written and `applied_at` is the
/// single shared stamp recorded for the transaction.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AppliedProposedChangeSet {
    pub run_id: RunId,
    /// One entry per applied change, in selection order.
    pub applied: Vec<AppliedProposedChange>,
    /// The logical-clock stamp recorded once for the whole transaction.
    pub applied_at: String,
}

/// An honest apply refusal. `conflict` distinguishes a baseline/content conflict
/// (the workspace moved out from under the change) from a structural refusal
/// (unsafe path, missing/irregular target) so the API can map it to the right
/// status.
#[derive(Debug)]
struct ApplyFailure {
    conflict: bool,
    reason: String,
}

/// An honest refusal of a **transactional** change-set apply. Like
/// [`ApplyFailure`], `conflict` marks a baseline/target conflict (the workspace
/// moved) vs a structural refusal (unsafe path, duplicate target, write failure),
/// so the API maps it to a 409 vs 422.
#[derive(Debug)]
struct ApplySetFailure {
    conflict: bool,
    reason: String,
}

/// One proposed change handed to the pure workspace writer: the filesystem
/// `action` (replace/create/rename/delete), the declared relative `path` (the
/// source for a rename, the target for a delete), the optional `dest` (the
/// destination for a rename; `None` otherwise), the optional `baseline` (Some for a
/// replace/rename/delete, None for a create), and the new `content` (empty for a
/// rename, which moves the file intact, and for a delete, which removes it).
#[derive(Debug)]
struct PlannedChange {
    action: relux_core::ProposedChangeAction,
    path: String,
    dest: Option<String>,
    baseline: Option<String>,
    content: String,
}

/// How the write phase must restore one already-written file when a later write
/// in the same transaction fails.
#[derive(Debug)]
enum RollbackPlan {
    /// The file existed before (a replace); restore these original bytes.
    RestoreOriginal(Vec<u8>),
    /// The file did not exist before (a create); delete it on rollback.
    DeleteCreated,
    /// The file was moved (a rename); move it back from the destination to the
    /// source. The source/destination paths live on the [`PlannedWrite`].
    RestoreRename,
    /// The file was removed (a delete); recreate it at its original path with these
    /// captured bytes. Content is restored as far as practical; file metadata
    /// (permissions, timestamps) is NOT preserved across the round-trip.
    RestoreDeleted(Vec<u8>),
}

/// One fully-validated change ready to write in a transactional apply, plus the
/// information the write phase needs to roll the file back if a later write in the
/// same transaction fails. For a rename, `safe_rel`/`target` are the *source* and
/// `dest_safe_rel`/`dest_target` are the *destination* the file moves to.
#[derive(Debug)]
struct PlannedWrite {
    action: relux_core::ProposedChangeAction,
    safe_rel: String,
    target: std::path::PathBuf,
    /// The destination path of a rename (`None` for replace/create).
    dest_safe_rel: Option<String>,
    /// The destination target of a rename (`None` for replace/create).
    dest_target: Option<std::path::PathBuf>,
    content: String,
    /// The path reported as applied (the destination for a rename, else `safe_rel`).
    report_rel: String,
    /// The bytes reported as written (the moved file size for a rename, else the
    /// new content length).
    report_bytes: u64,
    rollback: RollbackPlan,
}

/// The maximum size of an existing target file we will read to verify its
/// baseline hash. A file larger than this is refused rather than slurped into
/// memory — the proposed-change model is for ordinary text source files.
const APPLY_TARGET_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Resolve, conflict-check, and write one proposed change under `workspace_root`.
/// PURE of kernel state — it only touches the filesystem, so it can be reasoned
/// about (and tested) on its own. Every refusal is an [`ApplyFailure`] with an
/// honest reason; on success it returns the number of bytes written.
///
/// Safety (master plan section 17.5), common to both actions:
/// - the workspace root must exist and canonicalize (a controlled directory);
/// - the target must resolve *inside* that root with no `..` escape and no
///   symlink in any existing component (so a symlinked path can't redirect the
///   write outside the root).
///
/// For a **replace** (`baseline` = Some):
/// - the target must already exist as a regular file whose current SHA-256 equals
///   the declared `baseline` (else a conflict — the file is left untouched);
/// - the write is atomic (temp file in the same directory + rename), so a crash
///   mid-write never leaves a half-written file.
///
/// For a **create** (`baseline` = None):
/// - the target must NOT already exist (an existing path — file, dir, or symlink —
///   is a conflict; the existing thing is never modified);
/// - any missing parent directories are created (each component is a sanitized,
///   non-excluded, in-root name, and the existing prefix has no symlink — see the
///   walk above — so `create_dir_all` cannot be redirected outside the root);
/// - the file is placed atomically with an O_EXCL reservation (so a racing creator
///   loses) followed by a temp-file + rename (crash-atomic content).
///
/// For a **delete** (`baseline` = Some):
/// - the target must already exist as a regular file (never a directory or symlink)
///   whose current SHA-256 equals the declared `baseline` (else a conflict — the
///   file is left untouched);
/// - the file is removed (`std::fs::remove_file`); the reported size is the bytes
///   that were removed.
fn apply_change_to_workspace(
    workspace_root: &str,
    rel_path: &str,
    action: relux_core::ProposedChangeAction,
    baseline: Option<&str>,
    dest: Option<&str>,
    content: &str,
) -> Result<u64, ApplyFailure> {
    let refuse = |reason: String| ApplyFailure {
        conflict: false,
        reason,
    };
    let conflict = |reason: String| ApplyFailure {
        conflict: true,
        reason,
    };
    let lift = |TargetRefusal { conflict: c, reason }| ApplyFailure { conflict: c, reason };

    let (root_canon, safe_rel, target) =
        resolve_apply_target(workspace_root, rel_path).map_err(lift)?;

    match action {
        relux_core::ProposedChangeAction::Replace => {
            let baseline = baseline.ok_or_else(|| {
                refuse("internal: a replace reached the writer without a baseline".to_string())
            })?;
            let original = verify_replace_baseline(&target, &safe_rel, baseline)
                .map_err(|TargetRefusal { conflict: c, reason }| ApplyFailure { conflict: c, reason })?;
            let _ = original;
            let parent = target
                .parent()
                .ok_or_else(|| refuse("target has no parent directory".to_string()))?;
            let tmp = parent.join(format!(".relux-apply-{}.tmp", std::process::id()));
            std::fs::write(&tmp, content.as_bytes())
                .map_err(|e| refuse(format!("could not stage the new content: {e}")))?;
            if let Err(e) = std::fs::rename(&tmp, &target) {
                let _ = std::fs::remove_file(&tmp);
                return Err(refuse(format!("could not write {safe_rel}: {e}")));
            }
            Ok(content.len() as u64)
        }
        relux_core::ProposedChangeAction::Create => {
            // The target must not already exist (file, dir, or symlink).
            if std::fs::symlink_metadata(&target).is_ok() {
                return Err(conflict(format!(
                    "target {safe_rel} already exists; a create never overwrites an existing path"
                )));
            }
            ensure_parent_dirs(&root_canon, &safe_rel)
                .map_err(|TargetRefusal { conflict: c, reason }| ApplyFailure { conflict: c, reason })?;
            create_new_file_atomic(&target, &safe_rel, content, "0")
                .map_err(|TargetRefusal { conflict: c, reason }| ApplyFailure { conflict: c, reason })?;
            Ok(content.len() as u64)
        }
        relux_core::ProposedChangeAction::Rename => {
            let baseline = baseline.ok_or_else(|| {
                refuse("internal: a rename reached the writer without a baseline".to_string())
            })?;
            let dest_rel = dest.ok_or_else(|| {
                refuse("internal: a rename reached the writer without a destination".to_string())
            })?;
            // Source must be an existing regular file matching the baseline.
            let original = verify_replace_baseline(&target, &safe_rel, baseline).map_err(lift)?;
            // Destination must resolve safely inside the root and not yet exist.
            let (_, dest_safe_rel, dest_target) =
                resolve_apply_target(workspace_root, dest_rel).map_err(lift)?;
            if dest_safe_rel == safe_rel || dest_target == target {
                return Err(refuse(format!(
                    "rename source and destination are the same path: {safe_rel}"
                )));
            }
            if std::fs::symlink_metadata(&dest_target).is_ok() {
                return Err(conflict(format!(
                    "rename destination {dest_safe_rel} already exists; a rename never overwrites an existing path"
                )));
            }
            ensure_parent_dirs(&root_canon, &dest_safe_rel).map_err(lift)?;
            // Move the file. Source and destination are both inside the canonical
            // root (same filesystem), so the rename is atomic.
            if let Err(e) = std::fs::rename(&target, &dest_target) {
                return Err(refuse(format!(
                    "could not move {safe_rel} to {dest_safe_rel}: {e}"
                )));
            }
            Ok(original.len() as u64)
        }
        relux_core::ProposedChangeAction::Delete => {
            let baseline = baseline.ok_or_else(|| {
                refuse("internal: a delete reached the writer without a baseline".to_string())
            })?;
            // Target must be an existing regular file matching the baseline (a
            // directory/symlink/missing target is refused by this check).
            let original = verify_replace_baseline(&target, &safe_rel, baseline).map_err(lift)?;
            if let Err(e) = std::fs::remove_file(&target) {
                return Err(refuse(format!("could not delete {safe_rel}: {e}")));
            }
            // Report the size of the file that was removed.
            Ok(original.len() as u64)
        }
    }
}

/// An honest refusal from a shared apply helper, with the same `conflict` split as
/// [`ApplyFailure`]/[`ApplySetFailure`] so each caller can map it to its own type.
#[derive(Debug)]
struct TargetRefusal {
    conflict: bool,
    reason: String,
}

/// Canonicalize the workspace root and resolve `<root>/<rel>` for an apply,
/// refusing any unsafe/excluded path, any escape past the root, or any symlink in
/// an existing component. Returns `(canonical_root, safe_rel, target)`. Shared by
/// the single- and multi-file writers so the resolution rule is identical.
fn resolve_apply_target(
    workspace_root: &str,
    rel_path: &str,
) -> Result<(std::path::PathBuf, String, std::path::PathBuf), TargetRefusal> {
    let refuse = |reason: String| TargetRefusal {
        conflict: false,
        reason,
    };

    // The path was already sanitized at capture (relative, no `..`, not
    // excluded); re-validate defensively in case storage was tampered with.
    let safe_rel = relux_core::proposed_change::sanitize_change_path(rel_path)
        .ok_or_else(|| refuse(format!("unsafe or excluded target path: {rel_path}")))?;

    let root = std::path::Path::new(workspace_root);
    let root_canon = std::fs::canonicalize(root)
        .map_err(|e| refuse(format!("workspace root {workspace_root} is not accessible: {e}")))?;
    if !root_canon.is_dir() {
        return Err(refuse(format!(
            "workspace root {workspace_root} is not a directory"
        )));
    }

    // Resolve <root>/<rel>, refusing any escape or symlinked component.
    let mut target = root_canon.clone();
    for comp in safe_rel.split('/') {
        target.push(comp);
    }
    if !target.starts_with(&root_canon) {
        return Err(refuse(format!(
            "resolved path escapes the workspace root: {safe_rel}"
        )));
    }
    let mut cur = root_canon.clone();
    for comp in safe_rel.split('/') {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() => {
                return Err(refuse(format!("path crosses a symlink: {comp}")));
            }
            Ok(_) => {}
            // The first non-existent component ends the check. A replace then
            // treats a missing target as a conflict; a create requires it missing.
            Err(_) => break,
        }
    }
    Ok((root_canon, safe_rel, target))
}

/// Verify a replace target is an existing regular file (within the apply cap)
/// whose current SHA-256 equals `baseline`. Returns the original bytes (so a
/// transactional writer can roll back). A missing target or mismatch is a conflict.
fn verify_replace_baseline(
    target: &std::path::Path,
    safe_rel: &str,
    baseline: &str,
) -> Result<Vec<u8>, TargetRefusal> {
    let refuse = |reason: String| TargetRefusal {
        conflict: false,
        reason,
    };
    let conflict = |reason: String| TargetRefusal {
        conflict: true,
        reason,
    };

    let md = match std::fs::symlink_metadata(target) {
        Ok(md) => md,
        Err(_) => {
            return Err(conflict(format!(
                "target {safe_rel} does not exist; a replace applies over an existing baseline file \
                 (use a create action to add a new file)"
            )));
        }
    };
    if !md.file_type().is_file() {
        return Err(refuse(format!("target {safe_rel} is not a regular file")));
    }
    if md.len() > APPLY_TARGET_MAX_BYTES {
        return Err(refuse(format!(
            "target {safe_rel} is larger than the {APPLY_TARGET_MAX_BYTES}-byte apply cap"
        )));
    }
    let current = std::fs::read(target)
        .map_err(|e| refuse(format!("could not read target {safe_rel}: {e}")))?;
    let current_hash = relux_core::sha256_hex(&current);
    if current_hash != baseline {
        return Err(conflict(format!(
            "baseline mismatch for {safe_rel}: the file changed since the proposal \
             (expected {}, found {})",
            short_hash(baseline),
            short_hash(&current_hash)
        )));
    }
    Ok(current)
}

/// Create any missing parent directories for a create target. The existing prefix
/// was already verified to contain no symlink (see [`resolve_apply_target`]) and
/// every component is a sanitized, non-excluded, in-root name, so creating the
/// remaining directories cannot be redirected outside the root.
fn ensure_parent_dirs(root_canon: &std::path::Path, safe_rel: &str) -> Result<(), TargetRefusal> {
    let refuse = |reason: String| TargetRefusal {
        conflict: false,
        reason,
    };
    let mut dir = root_canon.to_path_buf();
    let comps: Vec<&str> = safe_rel.split('/').collect();
    // All components except the final file name are directories.
    for comp in &comps[..comps.len().saturating_sub(1)] {
        dir.push(comp);
        match std::fs::symlink_metadata(&dir) {
            Ok(md) if md.file_type().is_dir() => {}
            Ok(md) if md.file_type().is_symlink() => {
                return Err(refuse(format!("parent path crosses a symlink: {comp}")));
            }
            Ok(_) => {
                return Err(refuse(format!(
                    "parent path component {comp} exists but is not a directory"
                )));
            }
            Err(_) => {
                std::fs::create_dir(&dir).map_err(|e| {
                    refuse(format!("could not create parent directory {comp}: {e}"))
                })?;
            }
        }
    }
    Ok(())
}

/// Atomically place a brand-new file at `target` with `content`, never clobbering
/// an existing path. Reserves the path with an O_EXCL `create_new` open (so a
/// racing creator loses the race and we get an honest conflict), then stages the
/// content to a temp file in the same directory and renames it over the empty
/// reservation (crash-atomic content). `tmp_tag` disambiguates the temp name
/// within one transaction.
fn create_new_file_atomic(
    target: &std::path::Path,
    safe_rel: &str,
    content: &str,
    tmp_tag: &str,
) -> Result<(), TargetRefusal> {
    let refuse = |reason: String| TargetRefusal {
        conflict: false,
        reason,
    };
    let conflict = |reason: String| TargetRefusal {
        conflict: true,
        reason,
    };

    // Reserve the path: create_new fails atomically if anything is already there.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(conflict(format!(
                "target {safe_rel} already exists; a create never overwrites an existing path"
            )));
        }
        Err(e) => {
            return Err(refuse(format!("could not create {safe_rel}: {e}")));
        }
    }
    let parent = target
        .parent()
        .ok_or_else(|| refuse("target has no parent directory".to_string()))?;
    let tmp = parent.join(format!(".relux-create-{}-{}.tmp", std::process::id(), tmp_tag));
    if let Err(e) = std::fs::write(&tmp, content.as_bytes()) {
        let _ = std::fs::remove_file(target);
        return Err(refuse(format!("could not stage new content for {safe_rel}: {e}")));
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(target);
        return Err(refuse(format!("could not write {safe_rel}: {e}")));
    }
    Ok(())
}

/// First 8 chars of a hash, for a legible conflict message.
fn short_hash(h: &str) -> &str {
    &h[..h.len().min(8)]
}

/// Validate a **set** of proposed changes against `workspace_root` and write them
/// all transactionally (master plan section 15 / safety bar section 17.5). PURE
/// of kernel state — it only touches the filesystem. Each [`PlannedChange`] is a
/// replace (over an existing baseline file), a create (a new file), a rename (move
/// an existing baseline file), or a delete (remove an existing baseline file).
///
/// All-or-nothing in two phases:
///
/// 1. **Validate all** (no writes): each path must sanitize to a safe relative
///    path, resolve inside the canonical root with no `..`/symlink escape, and be a
///    distinct target (no two changes may write the same file; a rename occupies
///    BOTH its source and destination). A **replace** or **delete** must point at an
///    existing regular file (within the apply cap) whose current SHA-256 still
///    equals its declared baseline; its original bytes are captured so a later write
///    failure can restore them. A **create** must point at a path that does NOT yet
///    exist (else a conflict); its rollback is a delete. A **rename** verifies its
///    source like a replace and that its destination does not yet exist. ANY failure
///    returns before a single byte is written.
/// 2. **Write all** (replace: temp + rename; create: O_EXCL reservation + temp +
///    rename, with parent dirs created; rename: move; delete: remove): on the first
///    write error the already-written files are rolled back — replaces restored to
///    their captured originals, creates deleted, renames moved back, deletes
///    recreated from their captured bytes — and an honest failure is returned.
///    Because phase 1 is strict, this rollback path is essentially unreachable
///    except for a genuine mid-apply I/O fault.
///
/// On success returns one `(safe_rel, bytes_written)` per change, in input order.
fn apply_change_set_to_workspace(
    workspace_root: &str,
    changes: &[PlannedChange],
) -> Result<Vec<(String, u64)>, ApplySetFailure> {
    let refuse = |reason: String| ApplySetFailure {
        conflict: false,
        reason,
    };
    let conflict = |reason: String| ApplySetFailure {
        conflict: true,
        reason,
    };
    let lift = |TargetRefusal { conflict: c, reason }| ApplySetFailure { conflict: c, reason };

    if changes.is_empty() {
        return Err(refuse("no changes to apply".to_string()));
    }

    let root = std::path::Path::new(workspace_root);
    let root_canon = std::fs::canonicalize(root)
        .map_err(|e| refuse(format!("workspace root {workspace_root} is not accessible: {e}")))?;
    if !root_canon.is_dir() {
        return Err(refuse(format!(
            "workspace root {workspace_root} is not a directory"
        )));
    }

    // ── Phase 1: validate every change; capture originals. No writes. ──────────
    let mut planned: Vec<PlannedWrite> = Vec::with_capacity(changes.len());
    let mut seen_rel: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_target: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    for change in changes {
        let (_, safe_rel, target) =
            resolve_apply_target(workspace_root, &change.path).map_err(lift)?;

        // Every path a change occupies (the source/target, and a rename's
        // destination) must be distinct across the whole set — by safe rel and by
        // resolved path — so no two changes touch the same file in the same
        // transaction and apply order is never ambiguous.
        if !seen_rel.insert(safe_rel.clone()) || !seen_target.insert(target.clone()) {
            return Err(refuse(format!(
                "conflicting target path in the change set: {safe_rel}"
            )));
        }

        let mut dest_safe_rel: Option<String> = None;
        let mut dest_target: Option<std::path::PathBuf> = None;
        let mut report_rel = safe_rel.clone();
        let mut report_bytes = change.content.len() as u64;

        let rollback = match change.action {
            relux_core::ProposedChangeAction::Replace => {
                let baseline = change.baseline.as_deref().ok_or_else(|| {
                    refuse(format!(
                        "internal: replace {safe_rel} reached the set writer without a baseline"
                    ))
                })?;
                let original =
                    verify_replace_baseline(&target, &safe_rel, baseline).map_err(lift)?;
                RollbackPlan::RestoreOriginal(original)
            }
            relux_core::ProposedChangeAction::Create => {
                // The target must not already exist (file, dir, or symlink).
                if std::fs::symlink_metadata(&target).is_ok() {
                    return Err(conflict(format!(
                        "target {safe_rel} already exists; a create never overwrites an existing path"
                    )));
                }
                RollbackPlan::DeleteCreated
            }
            relux_core::ProposedChangeAction::Rename => {
                let baseline = change.baseline.as_deref().ok_or_else(|| {
                    refuse(format!(
                        "internal: rename {safe_rel} reached the set writer without a baseline"
                    ))
                })?;
                let dest_rel = change.dest.as_deref().ok_or_else(|| {
                    refuse(format!(
                        "internal: rename {safe_rel} reached the set writer without a destination"
                    ))
                })?;
                // Source must be an existing regular file matching the baseline.
                let original =
                    verify_replace_baseline(&target, &safe_rel, baseline).map_err(lift)?;
                // Destination must resolve safely inside the root and not yet exist.
                let (_, d_safe_rel, d_target) =
                    resolve_apply_target(workspace_root, dest_rel).map_err(lift)?;
                if d_safe_rel == safe_rel || d_target == target {
                    return Err(refuse(format!(
                        "rename source and destination are the same path: {safe_rel}"
                    )));
                }
                if std::fs::symlink_metadata(&d_target).is_ok() {
                    return Err(conflict(format!(
                        "rename destination {d_safe_rel} already exists; a rename never overwrites an existing path"
                    )));
                }
                // The destination is a second occupied path: it must also be unique
                // across the set (no other change may target or move onto it).
                if !seen_rel.insert(d_safe_rel.clone()) || !seen_target.insert(d_target.clone()) {
                    return Err(refuse(format!(
                        "conflicting target path in the change set: {d_safe_rel}"
                    )));
                }
                report_rel = d_safe_rel.clone();
                report_bytes = original.len() as u64;
                dest_safe_rel = Some(d_safe_rel);
                dest_target = Some(d_target);
                RollbackPlan::RestoreRename
            }
            relux_core::ProposedChangeAction::Delete => {
                let baseline = change.baseline.as_deref().ok_or_else(|| {
                    refuse(format!(
                        "internal: delete {safe_rel} reached the set writer without a baseline"
                    ))
                })?;
                // Target must be an existing regular file matching the baseline; its
                // original bytes are captured so a later write failure can restore it.
                let original =
                    verify_replace_baseline(&target, &safe_rel, baseline).map_err(lift)?;
                report_bytes = original.len() as u64;
                RollbackPlan::RestoreDeleted(original)
            }
        };

        planned.push(PlannedWrite {
            action: change.action,
            safe_rel,
            target,
            dest_safe_rel,
            dest_target,
            content: change.content.clone(),
            report_rel,
            report_bytes,
            rollback,
        });
    }

    // ── Phase 2: write every change, rolling back on failure. ──────────────────
    let mut written: Vec<&PlannedWrite> = Vec::with_capacity(planned.len());
    let mut applied: Vec<(String, u64)> = Vec::with_capacity(planned.len());
    for (i, p) in planned.iter().enumerate() {
        let write_res: Result<(), String> = match p.action {
            relux_core::ProposedChangeAction::Replace => {
                let parent = match p.target.parent() {
                    Some(parent) => parent,
                    None => {
                        let rolled = rollback_writes(&written);
                        return Err(refuse(rollback_message(
                            &format!("target {} has no parent directory", p.safe_rel),
                            &written,
                            rolled,
                        )));
                    }
                };
                let tmp = parent.join(format!(".relux-apply-{}-{}.tmp", std::process::id(), i));
                std::fs::write(&tmp, p.content.as_bytes())
                    .and_then(|()| std::fs::rename(&tmp, &p.target))
                    .map_err(|e| {
                        let _ = std::fs::remove_file(&tmp);
                        format!("could not write {}: {e}", p.safe_rel)
                    })
            }
            relux_core::ProposedChangeAction::Create => {
                // Parent dirs were not created in phase 1 (no writes there), so
                // create them now, then place the file atomically without clobber.
                ensure_parent_dirs(&root_canon, &p.safe_rel)
                    .and_then(|()| {
                        create_new_file_atomic(&p.target, &p.safe_rel, &p.content, &i.to_string())
                    })
                    .map_err(|TargetRefusal { reason, .. }| reason)
            }
            relux_core::ProposedChangeAction::Rename => {
                // Source was verified in phase 1; create the destination's parent
                // dirs (if any) and move the file. A missing dest field here is an
                // internal invariant break (phase 1 sets it for every rename).
                match (p.dest_safe_rel.as_deref(), p.dest_target.as_ref()) {
                    (Some(dest_safe_rel), Some(dest_target)) => {
                        ensure_parent_dirs(&root_canon, dest_safe_rel)
                            .map_err(|TargetRefusal { reason, .. }| reason)
                            .and_then(|()| {
                                std::fs::rename(&p.target, dest_target).map_err(|e| {
                                    format!(
                                        "could not move {} to {dest_safe_rel}: {e}",
                                        p.safe_rel
                                    )
                                })
                            })
                    }
                    _ => Err(format!(
                        "internal: rename {} reached the set writer without a destination",
                        p.safe_rel
                    )),
                }
            }
            relux_core::ProposedChangeAction::Delete => {
                // The target was verified in phase 1; remove it now. Its original
                // bytes are held on the rollback plan to restore on a later failure.
                std::fs::remove_file(&p.target)
                    .map_err(|e| format!("could not delete {}: {e}", p.safe_rel))
            }
        };
        if let Err(cause) = write_res {
            let rolled = rollback_writes(&written);
            return Err(refuse(rollback_message(&cause, &written, rolled)));
        }
        written.push(p);
        applied.push((p.report_rel.clone(), p.report_bytes));
    }
    Ok(applied)
}

/// Roll each already-written file in a failed transaction back to its pre-apply
/// state — a replace is restored to its captured original bytes (temp + rename); a
/// create is deleted; a rename is moved back to its source; a delete is recreated
/// from its captured bytes (content only — metadata is not preserved). Returns
/// `true` when every rollback succeeded.
fn rollback_writes(written: &[&PlannedWrite]) -> bool {
    let mut all_ok = true;
    for (i, p) in written.iter().enumerate() {
        match &p.rollback {
            RollbackPlan::RestoreOriginal(original) => {
                let parent = match p.target.parent() {
                    Some(parent) => parent,
                    None => {
                        all_ok = false;
                        continue;
                    }
                };
                let tmp =
                    parent.join(format!(".relux-rollback-{}-{}.tmp", std::process::id(), i));
                if std::fs::write(&tmp, original)
                    .and_then(|()| std::fs::rename(&tmp, &p.target))
                    .is_err()
                {
                    let _ = std::fs::remove_file(&tmp);
                    all_ok = false;
                }
            }
            RollbackPlan::DeleteCreated => {
                if std::fs::remove_file(&p.target).is_err() {
                    all_ok = false;
                }
            }
            RollbackPlan::RestoreRename => {
                // The file was moved source -> dest; move it back dest -> source.
                match &p.dest_target {
                    Some(dest_target) => {
                        if std::fs::rename(dest_target, &p.target).is_err() {
                            all_ok = false;
                        }
                    }
                    None => all_ok = false,
                }
            }
            RollbackPlan::RestoreDeleted(original) => {
                // The file was removed; recreate it at its source path with the
                // captured bytes (content restored as far as practical — metadata is
                // not preserved). The parent directory was never removed by a delete.
                let parent = match p.target.parent() {
                    Some(parent) => parent,
                    None => {
                        all_ok = false;
                        continue;
                    }
                };
                let tmp =
                    parent.join(format!(".relux-rollback-{}-{}.tmp", std::process::id(), i));
                if std::fs::write(&tmp, original)
                    .and_then(|()| std::fs::rename(&tmp, &p.target))
                    .is_err()
                {
                    let _ = std::fs::remove_file(&tmp);
                    all_ok = false;
                }
            }
        }
    }
    all_ok
}

/// Build an honest failure message for a mid-apply write error, stating whether
/// the rollback fully restored the prior writes (so the operator knows whether
/// the workspace is back to its pre-apply state or may have been left modified).
fn rollback_message(cause: &str, written: &[&PlannedWrite], rolled_back_ok: bool) -> String {
    if written.is_empty() {
        format!("{cause}; no files were written")
    } else if rolled_back_ok {
        format!(
            "{cause}; rolled back {} already-written file(s) to leave no net change",
            written.len()
        )
    } else {
        format!(
            "{cause}; ROLLBACK INCOMPLETE — {} file(s) were written and could not all be restored",
            written.len()
        )
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

/// Attach the one-click next-step buttons the chat surface renders for a turn
/// (`docs/RELUX_MASTER_PLAN.md` §11.1 "Prime suggested next actions").
///
/// Each suggestion is a pre-written user message routed through the normal
/// `prime_turn`, so a button can do nothing the user could not type by hand.
/// This is the single place suggestions are decided, keyed off the turn the
/// kernel already produced.
fn attach_suggestions(
    turn: &mut PrimeTurn,
    message: &str,
    summary: &relux_core::StateSummary,
) {
    use relux_core::{PrimeIntent, PrimeSuggestion};

    // After Prime mints a task that is ready but not yet running, offer to start
    // it as a button instead of telling the user what phrase to type.
    if turn.intent == PrimeIntent::TaskCreation
        && turn.created_task.is_some()
        && turn.started_run.is_none()
    {
        turn.suggested_actions.push(PrimeSuggestion {
            label: "Start the run".to_string(),
            message: "start it".to_string(),
            send: true,
        });
    }

    // Brainstorming stays a conversation (§10.5), but give the user a one-click
    // path to promote the idea into a task. The button pre-fills the command with
    // the work the message gestured at (`send: false`) so the user confirms or
    // edits the title - nothing is created until they send it.
    if turn.intent == PrimeIntent::Brainstorming {
        let candidate = brainstorm_task_candidate(message).unwrap_or_default();
        let prefill = if candidate.is_empty() {
            "create a task to ".to_string()
        } else {
            format!("create a task to {candidate}")
        };
        turn.suggested_actions.push(PrimeSuggestion {
            label: "Turn this into a task".to_string(),
            message: prefill,
            send: false,
        });
        // The middle rung of "idea -> plan -> tasks" (§10 planning layer, §11.1):
        // for an idea that is more than one piece of work, offer a one-click path
        // to a REVIEWABLE plan preview. The button pre-fills "plan out <idea>"
        // (`send: false`); the resulting turn creates nothing until the user
        // commits the plan, so musing flows into a plan without a magic phrase.
        let plan_prefill = if candidate.is_empty() {
            "plan out ".to_string()
        } else {
            format!("plan out {candidate}")
        };
        turn.suggested_actions.push(PrimeSuggestion {
            label: "Plan this out".to_string(),
            message: plan_prefill,
            send: false,
        });
    }

    // A plan request previews work but creates nothing (§10 planning layer, §11.1).
    // Offer the explicit one-click commit, keyed off the same decomposition the
    // preview showed: a multi-step plan routes the existing orchestration `Act`
    // ("Create these tasks"); a single-step goal is the one-task path. The message
    // is exactly what the user could type by hand, so the button is never a
    // privileged path - and nothing is created until they send it.
    if turn.intent == PrimeIntent::PlanRequest {
        let goal = plan_goal(message);
        let plan = relux_core::plan_orchestration(&goal, summary);
        let suggestion = if plan.is_multi_agent() {
            PrimeSuggestion {
                label: "Create these tasks".to_string(),
                message: format!("orchestrate {goal}"),
                send: false,
            }
        } else {
            PrimeSuggestion {
                label: "Turn this into a task".to_string(),
                message: format!("create a task to {goal}"),
                send: false,
            }
        };
        turn.suggested_actions.push(suggestion);
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
        PrimeAction::DiscoverTools => "list the installed tools".to_string(),
        PrimeAction::OrchestrateGoal { goal } => {
            format!("orchestrate \"{goal}\" across multiple agents")
        }
        PrimeAction::InvokeTool {
            plugin_id,
            tool_name,
            ..
        } => {
            if tool_name.is_empty() {
                format!("invoke the {plugin_id} tool")
            } else {
                format!("invoke {plugin_id}/{tool_name}")
            }
        }
        other => format!("{other:?}"),
    }
}

/// Render a grounded, multi-line summary of an orchestration Prime just created:
/// the goal, each brief with its assigned agent and role, any honest notes, and
/// the exact next step to run it. Speaks only about what was actually created.
fn render_orchestration_plan(o: &Orchestration) -> String {
    let mut s = format!(
        "I created orchestration {} for \"{}\" with {} briefs:",
        o.id,
        o.goal,
        o.steps.len()
    );
    for step in &o.steps {
        s.push_str(&format!(
            "\n  - {} \"{}\" -> {} ({})",
            step.task_id,
            step.title,
            step.agent_id,
            step.role.label()
        ));
    }
    for note in &o.notes {
        s.push_str(&format!("\n  note: {note}"));
    }
    s.push_str(&format!(
        "\nNothing is running yet. Start it from the Prime page's orchestration controls, or run `relux-kernel prime orchestration run {}` (CLI-adapter agents must have their runtime enabled first).",
        o.id
    ));
    s
}

/// Render a grounded tool-catalogue reply for a `DiscoverTools` turn.
///
/// Lists only tools the kernel actually discovered (it invents nothing), each
/// with its honest executable status: `ready` tools Prime can run now,
/// `not implemented` tools that are installed but have no local runtime, and
/// `needs permission` tools Prime would need a grant for. Disabled tools are
/// marked so the list never implies a disabled tool is runnable.
fn render_tool_catalog(intro: &str, tools: &[ToolDescriptor]) -> String {
    if tools.is_empty() {
        return "No tools are installed yet. Install a ToolSet plugin and I will list it here."
            .to_string();
    }
    let mut lines: Vec<String> = Vec::with_capacity(tools.len() + 1);
    lines.push(intro.trim().to_string());
    for t in tools {
        let status = match t.executable {
            ToolExecutability::Ready => "ready",
            ToolExecutability::RuntimeNotConfigured => {
                "installed, runtime not configured (set a loopback endpoint)"
            }
            ToolExecutability::RuntimeDisabled => "installed, runtime disabled",
            ToolExecutability::NotImplemented => "installed, runtime not implemented yet",
            ToolExecutability::MissingPermission => "needs permission",
        };
        let disabled = if t.enabled { "" } else { ", disabled" };
        lines.push(format!(
            "- {}/{} - {} [{}{}]",
            t.plugin_id, t.tool_name, t.description, status, disabled
        ));
    }
    lines.join("\n")
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

    fn status_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-tools-status"),
            name: "Status".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "control-plane status".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![ToolDefinition {
                    name: "status.summary".to_string(),
                    description: "deterministic control-plane counts".to_string(),
                    risk: RiskLevel::Low,
                    permission: Permission::new("tool:relux-tools-status:summary").unwrap(),
                    approval: ApprovalRequirement::Never,
                    timeout_secs: Some(5),
                }],
                permissions: vec![Permission::new("tool:relux-tools-status:summary").unwrap()],
            },
            health: PluginHealth::Unknown,
        }
    }

    /// A ToolSet plugin with NO built-in runtime handler - discoverable but not
    /// executable. Stands in for an installed-but-unimplemented tool.
    fn github_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-tools-github"),
            name: "GitHub".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "GitHub operations".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Community,
            capabilities: PluginCapability {
                tools: vec![ToolDefinition {
                    name: "github.create_pr".to_string(),
                    description: "open a pull request".to_string(),
                    risk: RiskLevel::High,
                    permission: Permission::new("tool:relux-tools-github:create_pr").unwrap(),
                    approval: ApprovalRequirement::Required,
                    timeout_secs: Some(30),
                }],
                permissions: vec![Permission::new("tool:relux-tools-github:create_pr").unwrap()],
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
    fn prime_create_agent_success() {
        let (mut k, ctx) = prime_chat_kernel();

        let turn = k
            .prime_turn(&ctx, "create an agent named researcher-bot")
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AgentCreation);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        let created_agent_id = turn.created_agent.expect("an agent was created");
        assert_eq!(created_agent_id.as_str(), "researcher-bot");
        assert_eq!(k.agent_count(), 2, "agent count should increase by 1"); // prime + researcher-bot
        assert!(k.agent(&created_agent_id).is_some());
    }

    #[test]
    fn prime_assign_task_success() {
        let (mut k, ctx) = prime_chat_kernel();

        // Create a task and an agent first
        let create_task_turn = k.prime_turn(&ctx, "create a task to research AI").unwrap();
        let task_id = create_task_turn.created_task.expect("task created");
        let create_agent_turn = k
            .prime_turn(&ctx, "create an agent named research-agent")
            .unwrap();
        let agent_id = create_agent_turn.created_agent.expect("agent created");

        let assign_turn = k
            .prime_turn(&ctx, &format!("assign {} to {}", task_id, agent_id))
            .unwrap();
        assert_eq!(assign_turn.intent, relux_core::PrimeIntent::AssignTask);
        assert_eq!(assign_turn.disposition, PrimeDisposition::Executed);
        assert!(assign_turn
            .reply
            .contains(&format!("Assigned task {} to agent {}", task_id, agent_id)));
        assert_eq!(
            k.task(&task_id).unwrap().assigned_agent.as_ref(),
            Some(&agent_id)
        );
    }

    #[test]
    fn prime_assign_task_clarifies_incomplete_intent() {
        let (mut k, ctx) = prime_chat_kernel();

        let turn = k.prime_turn(&ctx, "assign to research-agent").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AssignTask);
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.reply.contains("I couldn't find a task ID"));

        let turn = k.prime_turn(&ctx, "assign task_0001 to").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AssignTask);
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.reply.contains("I couldn't find an agent name"));

        let turn = k.prime_turn(&ctx, "assign").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AssignTask);
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn
            .reply
            .contains("I need both a task ID and an agent name"));
    }

    #[test]
    fn prime_assign_task_fails_for_non_existent_task_or_agent() {
        let (mut k, ctx) = prime_chat_kernel();

        // Non-existent task
        let turn = k.prime_turn(&ctx, "assign task_9999 to prime").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AssignTask);
        assert_eq!(turn.disposition, PrimeDisposition::Answered); // Disposition for reply
        assert!(turn
            .reply
            .contains("Task with ID 'task_9999' does not exist."));

        // Non-existent agent
        let create_task_turn = k.prime_turn(&ctx, "create a task to research AI").unwrap();
        let task_id = create_task_turn.created_task.expect("task created");
        let turn = k
            .prime_turn(&ctx, &format!("assign {} to missing-agent", task_id))
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AssignTask);
        assert_eq!(turn.disposition, PrimeDisposition::Answered); // Disposition for reply
        assert!(turn
            .reply
            .contains("Agent with ID 'missing-agent' does not exist."));
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
    fn run_events_since_returns_only_the_exclusive_tail() {
        // The incremental live-tail cursor: `run_events_since` must drop every
        // event up to AND INCLUDING the cursor, and return the rest in order.
        let (mut k, prime, task, run, echo) = primed_kernel();
        k.call_tool(
            &run,
            &prime,
            &echo,
            "echo.say",
            serde_json::json!({ "message": "hi" }),
        )
        .expect("tool call allowed");
        k.complete_run(&run, "done").unwrap();
        k.complete_task(&task).unwrap();

        // Full transcript: start -> tool_call -> completed.
        let full = k.run_events(&run);
        assert_eq!(full.len(), 3, "expected three transcript events");
        let first_id = full[0].id.clone();
        let second_id = full[1].id.clone();
        let third_id = full[2].id.clone();

        // A cursor at the first event returns only events 2 and 3.
        let tail = k.run_events_since(&run, Some(&first_id));
        let tail_kinds: Vec<&str> = tail.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(tail_kinds, vec!["tool_call", "run_completed"]);

        // A cursor at the last event returns an empty tail (nothing newer).
        assert!(k.run_events_since(&run, Some(&third_id)).is_empty());

        // The cursor is EXCLUSIVE: pointing at event 2 never re-emits event 2.
        let after_second = k.run_events_since(&run, Some(&second_id));
        assert_eq!(after_second.len(), 1);
        assert_eq!(after_second[0].kind, "run_completed");

        // None / empty / unparseable cursors all degrade to the full transcript.
        assert_eq!(k.run_events_since(&run, None).len(), 3);
        assert_eq!(k.run_events_since(&run, Some("")).len(), 3);
        assert_eq!(k.run_events_since(&run, Some("not-an-id")).len(), 3);
    }

    #[test]
    fn execute_local_run_completes_task_and_run() {
        let (mut k, _prime, task, run, _echo) = primed_kernel(); // primed_kernel sets up a run, but it's not completed by default anymore

        // Before calling execute_local_run, the run and task should be Running
        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Running);
        assert_eq!(k.run(&run).unwrap().status, RunStatus::Running);

        // Execute the run locally
        let completed_run_id = k.execute_local_run(&task).expect("local run should succeed");
        assert_eq!(completed_run_id, run);

        // After local execution, the run and task should be Completed
        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Completed);
        assert_eq!(k.run(&run).unwrap().status, RunStatus::Completed);

        // Verify run transcript
        let kinds: Vec<&str> = k.run_events(&run).iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, vec!["run_started", "tool_call", "run_completed"]);

        // Verify audit log for tool call and run completion
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "tool:relux-tools-echo:say" && e.result == AuditResult::Success));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "run:complete" && e.result == AuditResult::Success));

        // Negative test: calling execute_local_run on a completed task should fail
        let err = k.execute_local_run(&task).unwrap_err();
        assert!(matches!(err, KernelError::NoActiveRun(_)));
    }

    #[test]
    fn unpermissioned_start_run_is_denied_and_audited() {
        let mut k = KernelState::new();
        k.register_plugin(echo_manifest());
        k.register_plugin(adapter_manifest());        let ns = k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
        let adapter = PluginId::new("relux-adapter-local-prime");

        // Agent with no permissions
        let weak_agent = k
            .create_agent("weak", "Weak Agent", "no perms", &adapter, &ns, None, vec![])
            .unwrap();

        // Task with required permissions
        let echo_permission = Permission::new("tool:relux-tools-echo:say").unwrap();
        let task = k.create_task(
            "Task requiring echo",
            serde_json::json!({ "message": "hello" }),
            "founder",
            &ns,
            vec![echo_permission.clone()],
        );
        k.assign_task(&task, &weak_agent).unwrap();

        // Attempt to start run with unpermissioned agent
        let err = k.start_run(&task).unwrap_err();
        assert!(
            matches!(err, KernelError::PermissionDenied { .. }),
            "Expected PermissionDenied, got {:?}", err
        );

        // Verify audit log records denial
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "task:start_run" && e.result == AuditResult::Denied));

        // Agent with permission, task without assigned agent
        let _strong_agent = k
            .create_agent("strong", "Strong Agent", "has perms", &adapter, &ns, None, vec![echo_permission])
            .unwrap();
        let unassigned_task = k.create_task(
            "Unassigned task",
            serde_json::json!({ "message": "hello" }),
            "founder",
            &ns,
            vec![],
        );
        let err = k.start_run(&unassigned_task).unwrap_err();
        assert!(
            matches!(err, KernelError::TaskNotAssigned(_)),
            "Expected TaskNotAssigned, got {:?}", err
        );

        // Try execute_local_run on an unassigned task
        let err = k.execute_local_run(&unassigned_task).unwrap_err();
        assert!(
            matches!(err, KernelError::TaskNotAssigned(_)),
            "Expected TaskNotAssigned, got {:?}", err
        );

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
            .any(|e| e.action == "tool:relux-tools-echo:say" && e.result == AuditResult::Denied));
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

    /// A ToolSet manifest whose single tool has NO built-in kernel handler. Used
    /// to prove the kernel refuses to execute (and never fabricates output for) an
    /// installed-but-unimplemented tool.
    fn unsupported_toolset_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-tools-ghost"),
            name: "Ghost Tools".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "a tool with no runtime".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Community,
            capabilities: PluginCapability {
                tools: vec![ToolDefinition {
                    name: "ghost.act".to_string(),
                    description: "would do something if a runtime existed".to_string(),
                    risk: RiskLevel::Medium,
                    permission: Permission::new("tool:relux-tools-ghost:act").unwrap(),
                    approval: ApprovalRequirement::Never,
                    timeout_secs: Some(5),
                }],
                permissions: vec![Permission::new("tool:relux-tools-ghost:act").unwrap()],
            },
            health: PluginHealth::Unknown,
        }
    }

    #[test]
    fn unsupported_tool_is_refused_honestly_and_does_not_fabricate_output() {
        let (mut k, prime, _task, run, _echo) = primed_kernel();
        // Register a plugin whose tool has no built-in handler, and grant Prime
        // the permission so the refusal is about runtime support, not permission.
        k.register_plugin(unsupported_toolset_manifest());
        let ghost_perm = Permission::new("tool:relux-tools-ghost:act").unwrap();
        k.grant_permission_to_agent(&prime, ghost_perm).unwrap();
        let ghost = PluginId::new("relux-tools-ghost");

        let err = k
            .call_tool(&run, &prime, &ghost, "ghost.act", serde_json::json!({ "x": 1 }))
            .unwrap_err();
        assert!(
            matches!(err, KernelError::ToolRuntimeUnavailable { .. }),
            "got {err:?}"
        );

        // The transcript shows the honest failure marker, NOT a tool_call with
        // fabricated output.
        let kinds: Vec<&str> = k.run_events(&run).iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"tool_call_failed"), "got {kinds:?}");
        assert!(!kinds.contains(&"tool_call"), "must not fabricate a tool_call");

        // It is audited as a failure whose reason names the missing runtime.
        assert!(k.audit_log().iter().any(|e| e.action
            == "tool:relux-tools-ghost:act"
            && e.result == AuditResult::Failed
            && e.metadata["reason"]
                .as_str()
                .map(|r| r.contains("no runtime handler"))
                .unwrap_or(false)));
    }

    #[test]
    fn invoke_tool_runs_echo_and_audits_without_a_run() {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        let echo = PluginId::new("relux-tools-echo");

        let result = k
            .invoke_tool(&prime, &echo, "echo.say", serde_json::json!({ "hi": "there" }))
            .expect("echo invocation allowed");
        assert_eq!(result.output, serde_json::json!({ "hi": "there" }));
        assert_eq!(result.plugin_id, "relux-tools-echo");
        assert_eq!(result.tool_name, "echo.say");
        assert_eq!(result.agent_id, "prime");
        assert_eq!(result.permission, "tool:relux-tools-echo:say");

        // Audited as a success via the invoke path; no run/run-event was minted.
        assert!(k.audit_log().iter().any(|e| e.action == "tool:relux-tools-echo:say"
            && e.result == AuditResult::Success
            && e.metadata["via"] == "invoke"));
    }

    #[test]
    fn invoke_tool_denies_without_permission() {
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let weak = k
            .create_agent("weak", "Weak", "no perms", &adapter, &ns, None, vec![])
            .unwrap();
        let echo = PluginId::new("relux-tools-echo");

        let err = k
            .invoke_tool(&weak, &echo, "echo.say", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");
        assert!(k.audit_log().iter().any(|e| e.action == "tool:relux-tools-echo:say"
            && e.result == AuditResult::Denied));
    }

    #[test]
    fn invoke_tool_refuses_unsupported_runtime() {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        k.register_plugin(unsupported_toolset_manifest());
        k.grant_permission_to_agent(&prime, Permission::new("tool:relux-tools-ghost:act").unwrap())
            .unwrap();
        let ghost = PluginId::new("relux-tools-ghost");

        let err = k
            .invoke_tool(&prime, &ghost, "ghost.act", serde_json::json!({}))
            .unwrap_err();
        assert!(
            matches!(err, KernelError::ToolRuntimeUnavailable { .. }),
            "got {err:?}"
        );
    }

    /// Spawn a one-shot loopback HTTP server returning `response`, and return its
    /// `http://127.0.0.1:<port>` base URL.
    fn one_shot_http(response: &'static str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                // Drain the full request (headers + Content-Length body) before
                // responding, so the server never closes the socket mid-write.
                let mut data: Vec<u8> = Vec::new();
                let mut buf = [0u8; 4096];
                let header_end = loop {
                    if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    match sock.read(&mut buf) {
                        Ok(0) => break data.len(),
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                        Err(_) => break data.len(),
                    }
                };
                let headers = String::from_utf8_lossy(&data[..header_end]).to_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while data.len() - header_end < content_length {
                    match sock.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                        Err(_) => break,
                    }
                }
                let _ = sock.write_all(response.as_bytes());
                let _ = sock.flush();
            }
        });
        base
    }

    /// Install ghost as a removable LocalDir plugin and grant Prime its permission.
    fn primed_with_installed_ghost() -> (KernelState, AgentId, PluginId) {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        k.install_plugin(
            unsupported_toolset_manifest(),
            PluginSourceKind::LocalDir,
            "/tmp/ghost".to_string(),
            "/data/ghost".to_string(),
            true,
        );
        k.grant_permission_to_agent(&prime, Permission::new("tool:relux-tools-ghost:act").unwrap())
            .unwrap();
        (k, prime, PluginId::new("relux-tools-ghost"))
    }

    #[test]
    fn configured_runtime_invokes_manifest_tool_and_returns_output() {
        let (mut k, prime, ghost) = primed_with_installed_ghost();
        let base = one_shot_http(
            "HTTP/1.1 200 OK\r\nContent-Length: 31\r\nConnection: close\r\n\r\n{\"output\":{\"pong\":true}}\n\n\n\n",
        );
        k.configure_tool_runtime(&ghost, &base, true, Some(2_000))
            .expect("configure runtime");

        let result = k
            .invoke_tool(&prime, &ghost, "ghost.act", serde_json::json!({ "ping": 1 }))
            .expect("loopback invocation ok");
        assert_eq!(result.output, serde_json::json!({ "pong": true }));
        // Audited as a success through the invoke path.
        assert!(k.audit_log().iter().any(|e| e.action
            == "tool:relux-tools-ghost:act"
            && e.result == AuditResult::Success));
    }

    #[test]
    fn disabled_runtime_refuses_invocation() {
        let (mut k, prime, ghost) = primed_with_installed_ghost();
        k.configure_tool_runtime(&ghost, "http://127.0.0.1:19999", true, None)
            .unwrap();
        k.disable_tool_runtime(&ghost).unwrap();

        let err = k
            .invoke_tool(&prime, &ghost, "ghost.act", serde_json::json!({}))
            .unwrap_err();
        assert!(
            matches!(err, KernelError::ToolRuntimeDisabled { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn permission_denied_happens_before_the_http_call() {
        // Configure a runtime that points at a dead loopback port. A denied agent
        // must fail with PermissionDenied, NOT a connect/timeout error - proving
        // the permission gate runs before any HTTP work.
        let (mut k, _prime, ghost) = primed_with_installed_ghost();
        k.configure_tool_runtime(&ghost, "http://127.0.0.1:1", true, Some(500))
            .unwrap();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let weak = k
            .create_agent("weak", "Weak", "no perms", &adapter, &ns, None, vec![])
            .unwrap();

        let err = k
            .invoke_tool(&weak, &ghost, "ghost.act", serde_json::json!({}))
            .unwrap_err();
        assert!(
            matches!(err, KernelError::PermissionDenied { .. }),
            "permission must be checked before the HTTP call, got {err:?}"
        );
    }

    #[test]
    fn runtime_returning_error_payload_is_surfaced_honestly() {
        let (mut k, prime, ghost) = primed_with_installed_ghost();
        let base = one_shot_http(
            "HTTP/1.1 200 OK\r\nContent-Length: 24\r\nConnection: close\r\n\r\n{\"error\":\"nope\"}\n\n\n\n\n\n",
        );
        k.configure_tool_runtime(&ghost, &base, true, Some(2_000))
            .unwrap();
        let err = k
            .invoke_tool(&prime, &ghost, "ghost.act", serde_json::json!({}))
            .unwrap_err();
        match err {
            KernelError::ToolRuntimeInvocation { message, .. } => {
                assert!(message.contains("nope"), "got {message}")
            }
            other => panic!("expected ToolRuntimeInvocation, got {other:?}"),
        }
        // Audited as a failure - never a fabricated success.
        assert!(k.audit_log().iter().any(|e| e.action
            == "tool:relux-tools-ghost:act"
            && e.result == AuditResult::Failed));
    }

    #[test]
    fn runtime_config_rejects_bundled_and_bad_url_and_uninstalled() {
        let (mut k, _prime, ghost) = primed_with_installed_ghost();
        // Bundled echo cannot get a loopback runtime.
        k.install_plugin(
            echo_manifest(),
            PluginSourceKind::Bundled,
            "bundled".to_string(),
            "examples/relux-plugins/relux-tools-echo".to_string(),
            true,
        );
        let echo = PluginId::new("relux-tools-echo");
        let err = k
            .configure_tool_runtime(&echo, "http://127.0.0.1:19999", true, None)
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidRuntimeConfig { .. }), "got {err:?}");

        // A non-loopback URL is refused for an installed plugin.
        let err = k
            .configure_tool_runtime(&ghost, "https://example.com:443", true, None)
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidRuntimeConfig { .. }), "got {err:?}");

        // An uninstalled plugin cannot be configured.
        let err = k
            .configure_tool_runtime(
                &PluginId::new("relux-tools-nope"),
                "http://127.0.0.1:19999",
                true,
                None,
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::PluginNotInstalled(_)), "got {err:?}");
    }

    #[test]
    fn runtime_config_survives_snapshot_roundtrip_and_clears_on_remove() {
        let (mut k, _prime, ghost) = primed_with_installed_ghost();
        k.configure_tool_runtime(&ghost, "http://127.0.0.1:19999", true, Some(1_234))
            .unwrap();

        // Snapshot -> restore preserves the config.
        let restored = KernelState::from_snapshot(k.snapshot());
        let cfg = restored.tool_runtime_config(&ghost).expect("config survives");
        assert_eq!(cfg.base_url, "http://127.0.0.1:19999");
        assert_eq!(cfg.timeout_ms, 1_234);
        assert!(cfg.enabled);

        // Removing the installed plugin clears its runtime config.
        k.remove_installed_plugin(&ghost).unwrap();
        assert!(k.tool_runtime_config(&ghost).is_none());
    }

    #[test]
    fn discover_tools_marks_ready_vs_runtime_status() {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        // primed_kernel only registers manifests; install records back discovery,
        // so install echo + the unsupported plugin as records.
        k.install_plugin(
            echo_manifest(),
            PluginSourceKind::Bundled,
            "bundled".to_string(),
            "examples/relux-plugins/relux-tools-echo".to_string(),
            true,
        );
        k.install_plugin(
            unsupported_toolset_manifest(),
            PluginSourceKind::LocalDir,
            "/tmp/ghost".to_string(),
            "/data/ghost".to_string(),
            true,
        );

        // Without an agent context: echo is ready (built-in), ghost has no runtime
        // configured yet.
        let tools = k.discover_tools(None);
        let echo = tools
            .iter()
            .find(|t| t.tool_name == "echo.say")
            .expect("echo discovered");
        assert_eq!(echo.executable, ToolExecutability::Ready);
        assert!(echo.protected, "bundled echo is protected");
        let ghost = tools
            .iter()
            .find(|t| t.tool_name == "ghost.act")
            .expect("ghost discovered");
        assert_eq!(ghost.executable, ToolExecutability::RuntimeNotConfigured);
        assert!(!ghost.protected);

        // Configure + enable an HTTP loopback runtime for ghost: it becomes ready.
        let ghost_id = PluginId::new("relux-tools-ghost");
        k.configure_tool_runtime(&ghost_id, "http://127.0.0.1:19999", true, None)
            .expect("configure runtime");
        let tools = k.discover_tools(None);
        let ghost = tools.iter().find(|t| t.tool_name == "ghost.act").unwrap();
        assert_eq!(ghost.executable, ToolExecutability::Ready);

        // Disable it: it flips to runtime_disabled.
        k.disable_tool_runtime(&ghost_id).unwrap();
        let tools = k.discover_tools(None);
        let ghost = tools.iter().find(|t| t.tool_name == "ghost.act").unwrap();
        assert_eq!(ghost.executable, ToolExecutability::RuntimeDisabled);

        // Re-enable for the permission-scoping checks below.
        k.configure_tool_runtime(&ghost_id, "http://127.0.0.1:19999", true, None)
            .unwrap();

        // Scoped to an agent WITHOUT the echo permission: echo flips to
        // missing_permission; ghost (enabled runtime, no perm) also flips.
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let weak = k
            .create_agent("weak", "Weak", "no perms", &adapter, &ns, None, vec![])
            .unwrap();
        let scoped = k.discover_tools(Some(&weak));
        let echo = scoped.iter().find(|t| t.tool_name == "echo.say").unwrap();
        assert_eq!(echo.executable, ToolExecutability::MissingPermission);
        let ghost = scoped.iter().find(|t| t.tool_name == "ghost.act").unwrap();
        assert_eq!(ghost.executable, ToolExecutability::MissingPermission);

        // Scoped to Prime (which holds the echo permission): echo is ready.
        let primed = k.discover_tools(Some(&prime));
        let echo = primed.iter().find(|t| t.tool_name == "echo.say").unwrap();
        assert_eq!(echo.executable, ToolExecutability::Ready);
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
        // Install (not just register) the bundled tools so capability discovery
        // and Prime's tool invocation see them, exactly like `ensure_bootstrapped`.
        install_bundled(&mut k, echo_manifest());
        install_bundled(&mut k, status_manifest());
        install_bundled(&mut k, adapter_manifest());
        let ns = k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
        let adapter = PluginId::new("relux-adapter-local-prime");
        // Prime holds exactly the two safe built-in tool permissions, matching the
        // production bootstrap (least privilege).
        let prime = k
            .create_agent(
                "prime",
                "Prime",
                "The control-plane operator.",
                &adapter,
                &ns,
                None,
                vec![
                    Permission::new("tool:relux-tools-echo:say").unwrap(),
                    Permission::new("tool:relux-tools-status:summary").unwrap(),
                ],
            )
            .unwrap();
        let ctx = PrimeContext {
            namespace: ns,
            agent: prime,
            actor: "founder".to_string(),
        };
        (k, ctx)
    }

    /// Install a manifest as a bundled, enabled plugin (test convenience).
    fn install_bundled(k: &mut KernelState, manifest: PluginManifest) {
        let id = manifest.id.as_str().to_string();
        k.install_plugin(
            manifest,
            PluginSourceKind::Bundled,
            "bundled example".to_string(),
            format!("examples/relux-plugins/{id}"),
            true,
        );
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
        // start it now only starts the run, does not complete it.
        assert_eq!(k.run(&run_id).unwrap().status, RunStatus::Running);
        assert_eq!(k.task(&task_id).unwrap().status, TaskStatus::Running);
    }

    #[test]
    fn prime_suggests_next_actions_per_doc_11_1() {
        // §11.1 "Prime suggested next actions": creating a task offers a one-click
        // "Start the run" that sends the real command, and brainstorming offers a
        // "Turn this into a task" that PRE-FILLS the command (send=false) so the
        // user confirms the title. A suggestion is never a privileged path - it is
        // just a pre-written user message.
        let (mut k, ctx) = prime_chat_kernel();

        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        let start = created
            .suggested_actions
            .iter()
            .find(|s| s.label == "Start the run")
            .expect("task creation offers a start button");
        assert_eq!(start.message, "start it");
        assert!(start.send, "starting the run sends immediately");

        // Brainstorming stays a conversation (no work minted) but offers the
        // promote-to-task button, pre-filled with the work it gestured at.
        let before = k.task_count();
        let muse = k
            .prime_turn(&ctx, "I was thinking we could redo the onboarding flow")
            .unwrap();
        assert_eq!(muse.disposition, PrimeDisposition::Answered);
        assert_eq!(k.task_count(), before, "brainstorming must not create work");
        let promote = muse
            .suggested_actions
            .iter()
            .find(|s| s.label == "Turn this into a task")
            .expect("brainstorming offers a promote-to-task button");
        assert_eq!(promote.message, "create a task to redo the onboarding flow");
        assert!(!promote.send, "promoting an idea pre-fills, never auto-sends");
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

        assert_eq!(k.task(&task_id).unwrap().status, TaskStatus::Running);
        assert_eq!(k.run(&run_id).unwrap().status, RunStatus::Running);

        // Transcript shows run_started event, but no tool_call or run_completed yet
        let kinds: Vec<&str> = k
            .run_events(&run_id)
            .iter()
            .map(|e| e.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["run_started"]);
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

        k.resolve_approval(&approval, true, "founder", None)
            .unwrap();
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
    fn prime_autonomy_config_defaults() {
        let config = PrimeAutonomyConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.interval_seconds, 60);
        assert_eq!(config.max_tasks_per_tick, 1);
        assert!(!config.auto_assign_unassigned);
        assert!(config.last_tick_at.is_none());
        assert!(config.last_tick_summary.is_none());
    }

    #[test]
    fn prime_autonomy_config_persistence() {
        let (mut k, _ctx) = prime_chat_kernel();
        let now = "2026-06-08T00:00:00Z".to_string();

        // Modify config
        k.prime_autonomy_config.enabled = true;
        k.prime_autonomy_config.interval_seconds = 120;
        k.prime_autonomy_config.max_tasks_per_tick = 5;
        k.prime_autonomy_config.auto_assign_unassigned = true;
        k.prime_autonomy_config.last_tick_at = Some(now.clone());
        k.prime_autonomy_config.last_tick_summary = Some("Test summary".to_string());

        let snapshot = k.snapshot();
        let restored_k = KernelState::from_snapshot(snapshot);

        let restored_config = restored_k.prime_autonomy_config;
        assert!(restored_config.enabled);
        assert_eq!(restored_config.interval_seconds, 120);
        assert_eq!(restored_config.max_tasks_per_tick, 5);
        assert!(restored_config.auto_assign_unassigned);
        assert_eq!(restored_config.last_tick_at, Some(now));
        assert_eq!(restored_config.last_tick_summary, Some("Test summary".to_string()));
    }

    #[test]
    fn one_autonomy_tick_disabled() {
        let (mut k, _ctx) = prime_chat_kernel();
        k.prime_autonomy_config.enabled = false; // Ensure it's disabled

        let result = k.one_autonomy_tick();

        assert!(!k.prime_autonomy_config.enabled);
        assert_eq!(result.summary, "Autonomy is disabled.".to_string());
        assert_eq!(result.actions_taken, 0);
        assert!(k.prime_autonomy_config.last_tick_at.is_some());
        assert_eq!(
            k.prime_autonomy_config.last_tick_summary,
            Some("Autonomy is disabled.".to_string())
        );

        // Verify audit event
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "autonomy:tick_skipped"
                && e.result == AuditResult::Denied
                && e.metadata["reason"] == "autonomy disabled"));
    }

    #[test]
    fn one_autonomy_tick_prime_not_found() {
        let mut k = KernelState::new(); // No prime agent created
        k.prime_autonomy_config.enabled = true;

        let result = k.one_autonomy_tick();

        assert_eq!(result.summary, "Prime agent not found.".to_string());
        assert_eq!(result.actions_taken, 0);
        assert!(k.prime_autonomy_config.last_tick_at.is_some());
        assert_eq!(
            k.prime_autonomy_config.last_tick_summary,
            Some("Prime agent not found.".to_string())
        );

        // Verify audit event
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "autonomy:tick_skipped"
                && e.result == AuditResult::Denied
                && e.metadata["reason"] == "prime agent not found"));
    }

    #[test]
    fn one_autonomy_tick_runs_assigned_tasks() {
        let (mut k, ctx) = prime_chat_kernel();
        k.prime_autonomy_config.enabled = true;
        k.prime_autonomy_config.max_tasks_per_tick = 1;

        // Create a queued task assigned to Prime
        let task1 = k.create_task("Task 1", serde_json::json!({}), "founder", &ctx.namespace, vec![]);
        k.assign_task(&task1, &ctx.agent).unwrap();

        // Create another queued task assigned to Prime, which should be skipped due to max_tasks_per_tick
        let task2 = k.create_task("Task 2", serde_json::json!({}), "founder", &ctx.namespace, vec![]);
        k.assign_task(&task2, &ctx.agent).unwrap();

        let result = k.one_autonomy_tick();

        assert_eq!(result.actions_taken, 1);
        assert_eq!(result.tasks_run, 1);
        assert_eq!(result.tasks_assigned, 0); // No tasks assigned
        assert!(k.task(&task1).unwrap().status == TaskStatus::Completed); // Task 1 should be completed
        assert!(k.task(&task2).unwrap().status == TaskStatus::Queued); // Task 2 should still be queued
        assert!(result.summary.contains("1 task(s) run, 0 task(s) assigned."));

        // Verify audit events
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "autonomy:run_completed" && e.result == AuditResult::Success));
    }

    #[test]
    fn one_autonomy_tick_auto_assigns_unassigned_tasks() {
        let (mut k, ctx) = prime_chat_kernel();
        k.prime_autonomy_config.enabled = true;
        k.prime_autonomy_config.max_tasks_per_tick = 1;
        k.prime_autonomy_config.auto_assign_unassigned = true;

        // Create an unassigned queued task
        let task1 = k.create_task("Unassigned Task 1", serde_json::json!({}), "founder", &ctx.namespace, vec![]);
        let task_obj1 = k.tasks.get_mut(&task1).unwrap();
        task_obj1.status = TaskStatus::Queued;
        task_obj1.assigned_agent = None;

        // Create another unassigned queued task, to be skipped
        let task2 = k.create_task("Unassigned Task 2", serde_json::json!({}), "founder", &ctx.namespace, vec![]);
        let task_obj2 = k.tasks.get_mut(&task2).unwrap();
        task_obj2.status = TaskStatus::Queued;
        task_obj2.assigned_agent = None;


        let result = k.one_autonomy_tick();

        assert_eq!(result.actions_taken, 2);
        assert_eq!(result.tasks_run, 1);
        assert_eq!(result.tasks_assigned, 1); // Task 1 should be assigned
        assert!(k.task(&task1).unwrap().assigned_agent.is_some());
        assert!(k.task(&task1).unwrap().status == TaskStatus::Completed);
        assert!(k.task(&task2).unwrap().assigned_agent.is_none());
        assert!(k.task(&task2).unwrap().status == TaskStatus::Queued);
        assert!(result.summary.contains("1 task(s) run, 1 task(s) assigned."));

        // Verify audit events
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "autonomy:task_assigned"
                && e.target_id.as_deref() == Some(task1.as_str())
                && e.result == AuditResult::Success));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "autonomy:run_completed"
                && e.metadata["task"] == task1.as_str()
                && e.result == AuditResult::Success));
    }

    #[test]
    fn one_autonomy_tick_handles_mix_of_assigned_and_unassigned() {
        let (mut k, ctx) = prime_chat_kernel();
        k.prime_autonomy_config.enabled = true;
        k.prime_autonomy_config.max_tasks_per_tick = 2; // Can handle 2 actions
        k.prime_autonomy_config.auto_assign_unassigned = true;

        // Task 1: Already assigned, will be run
        let task1 = k.create_task("Assigned Task", serde_json::json!({}), "founder", &ctx.namespace, vec![]);
        k.assign_task(&task1, &ctx.agent).unwrap();

        // Task 2: Unassigned, will be auto-assigned
        let task2 = k.create_task("Unassigned Task", serde_json::json!({}), "founder", &ctx.namespace, vec![]);
        let task_obj2 = k.tasks.get_mut(&task2).unwrap();
        task_obj2.status = TaskStatus::Queued;
        task_obj2.assigned_agent = None;

        // Task 3: Another unassigned, will be skipped due to max_tasks_per_tick
        let task3 = k.create_task("Another Unassigned Task", serde_json::json!({}), "founder", &ctx.namespace, vec![]);
        let task_obj3 = k.tasks.get_mut(&task3).unwrap();
        task_obj3.status = TaskStatus::Queued;
        task_obj3.assigned_agent = None;

        let result = k.one_autonomy_tick();

        assert_eq!(result.actions_taken, 3);
        assert_eq!(result.tasks_run, 2);
        assert_eq!(result.tasks_assigned, 1);
        assert!(result.summary.contains("2 task(s) run, 1 task(s) assigned."));

        assert!(k.task(&task1).unwrap().status == TaskStatus::Completed); // Task 1 ran
        assert!(k.task(&task2).unwrap().assigned_agent.is_some()); // Task 2 assigned
        assert!(k.task(&task2).unwrap().status == TaskStatus::Completed); // Task 2 ran after assignment
        assert!(k.task(&task3).unwrap().assigned_agent.is_none()); // Task 3 skipped
        assert!(k.task(&task3).unwrap().status == TaskStatus::Queued);
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

    // --- Prime tool awareness (master plan §11.1, Tool Invocation Surface) ----

    #[test]
    fn greeting_does_not_invoke_a_tool() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "hey").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::Greeting);
        assert!(turn.invoked_tool.is_none(), "a greeting must not run a tool");
        assert!(turn.tool_output.is_none());
        assert!(turn.tool_error.is_none());
        assert_eq!(k.run_count(), 0, "a greeting must not start a run");
    }

    #[test]
    fn tool_discovery_lists_only_grounded_installed_tools() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "what tools can you use?").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::ToolDiscovery);
        assert_eq!(turn.disposition, PrimeDisposition::Answered);
        // The genuine read-only status tool is listed, with honest status.
        assert!(
            turn.reply.contains("relux-tools-status/status.summary"),
            "got: {}",
            turn.reply
        );
        assert!(turn.reply.contains("ready"), "ready tools marked: {}", turn.reply);
        // The internal echo fixture is HIDDEN from the user-facing catalogue so
        // Prime never offers it as a real ability.
        assert!(
            !turn.reply.contains("echo.say"),
            "echo must be hidden from Prime's tool catalogue: {}",
            turn.reply
        );
        // It must not fabricate a tool that is not installed.
        assert!(!turn.reply.contains("github"), "no fabricated tools: {}", turn.reply);
        assert!(turn.invoked_tool.is_none(), "discovery lists, it does not invoke");
    }

    #[test]
    fn status_request_invokes_status_summary_tool() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "give me a status summary").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::StatusQuestion);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        assert_eq!(
            turn.invoked_tool.as_deref(),
            Some("relux-tools-status/status.summary")
        );
        // The real tool output is present and is the grounded state summary.
        let output = turn.tool_output.expect("status tool returned output");
        assert!(output.get("tasks_total").is_some(), "got: {output}");
        assert!(turn.tool_error.is_none());
    }

    #[test]
    fn echo_request_invokes_echo_say_with_input_and_output() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "echo hello").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::ToolInvocation);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        assert_eq!(
            turn.invoked_tool.as_deref(),
            Some("relux-tools-echo/echo.say")
        );
        // echo.say returns its input unchanged; the input was the message text.
        assert_eq!(
            turn.tool_output,
            Some(serde_json::json!({ "message": "hello" }))
        );
        assert!(turn.tool_error.is_none());
    }

    #[test]
    fn echo_request_accepts_inline_json_input() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k
            .prime_turn(&ctx, "use echo.say with {\"n\": 7}")
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::ToolInvocation);
        assert_eq!(
            turn.invoked_tool.as_deref(),
            Some("relux-tools-echo/echo.say")
        );
        assert_eq!(turn.tool_output, Some(serde_json::json!({ "n": 7 })));
    }

    #[test]
    fn unsupported_installed_tool_is_reported_not_fabricated() {
        let (mut k, ctx) = prime_chat_kernel();
        // Install a real ToolSet plugin with no built-in runtime handler.
        install_bundled(&mut k, github_manifest());
        let turn = k.prime_turn(&ctx, "use the github tool").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::ToolInvocation);
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        // It is honest: installed/discoverable but no local runtime, and NO output.
        assert!(turn.invoked_tool.is_none(), "nothing actually ran");
        assert!(turn.tool_output.is_none(), "no fabricated output");
        let err = turn.tool_error.expect("an honest tool_error");
        assert!(err.contains("relux-tools-github"), "got: {err}");
        assert!(
            err.contains("cannot execute it yet")
                || err.contains("not implemented")
                || err.contains("no runtime configured"),
            "got: {err}"
        );
    }

    #[test]
    fn tool_invocation_denied_when_prime_lacks_permission() {
        // A Prime that holds NO echo permission cannot run the (built-in) echo tool.
        let mut k = KernelState::new();
        install_bundled(&mut k, echo_manifest());
        install_bundled(&mut k, adapter_manifest());
        let ns = k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
        let adapter = PluginId::new("relux-adapter-local-prime");
        let prime = k
            .create_agent("prime", "Prime", "op", &adapter, &ns, None, vec![])
            .unwrap();
        let ctx = PrimeContext {
            namespace: ns,
            agent: prime,
            actor: "founder".to_string(),
        };
        let turn = k.prime_turn(&ctx, "echo hello").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::ToolInvocation);
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.invoked_tool.is_none(), "denied call must not run");
        assert!(turn.tool_output.is_none(), "no fabricated output on denial");
        let err = turn.tool_error.expect("an honest tool_error");
        assert!(err.contains("permission"), "got: {err}");
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
        assert_eq!(restored.task(&task).unwrap().status, TaskStatus::Running);
        assert_eq!(restored.run(&run).unwrap().status, RunStatus::Running);
        assert_eq!(restored.run_events(&run).len(), k.run_events(&run).len());
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

    #[test]
    fn grant_permission_to_agent_works_and_audits() {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        let new_permission = Permission::new("tool:relux-tools-github:read").unwrap();

        // Grant a new permission
        k.grant_permission_to_agent(&prime, new_permission.clone())
            .expect("should grant new permission");
        let updated_prime = k.agent(&prime).unwrap();
        assert!(updated_prime.permissions.contains(&new_permission));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "agent:grant_permission" && e.result == AuditResult::Success));

        // Try to grant the same permission again
        let err = k
            .grant_permission_to_agent(&prime, new_permission.clone())
            .unwrap_err();
        assert!(
            matches!(err, KernelError::PermissionAlreadyGranted(..)),
            "got {err:?}"
        );
    }

    // --- Adapter runtime tests --------------------------------------------

    fn claude_adapter_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("relux-adapter-claude-cli"),
            name: "Claude CLI".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::Adapter,
            description: "Claude CLI adapter".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![],
                permissions: vec![
                    Permission::new("adapter:relux-adapter-claude-cli:run").unwrap()
                ],
            },
            health: PluginHealth::Unknown,
        }
    }

    /// Install the echo, local-prime, and claude-cli adapters as Bundled, create
    /// the workspace namespace, and return the kernel.
    fn adapter_kernel() -> KernelState {
        let mut k = KernelState::new();
        k.install_plugin(
            echo_manifest(),
            PluginSourceKind::Bundled,
            "bundled".to_string(),
            "examples/relux-plugins/relux-tools-echo".to_string(),
            true,
        );
        k.install_plugin(
            adapter_manifest(),
            PluginSourceKind::Bundled,
            "bundled".to_string(),
            "examples/relux-plugins/relux-adapter-local-prime".to_string(),
            true,
        );
        k.install_plugin(
            claude_adapter_manifest(),
            PluginSourceKind::Bundled,
            "bundled".to_string(),
            "examples/relux-plugins/relux-adapter-claude-cli".to_string(),
            true,
        );
        k.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
        k
    }

    /// Write a fake CLI that prints `output` and exits 0 (cross-platform).
    fn write_fake_cli(dir: &std::path::Path, name: &str, output: &str) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            std::fs::write(&path, format!("@echo off\r\necho {output}\r\n")).unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\necho '{output}'\n")).unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    #[test]
    fn configure_adapter_runtime_rejects_local_prime() {
        let mut k = adapter_kernel();
        let err = k
            .configure_adapter_runtime(
                &PluginId::new("relux-adapter-local-prime"),
                Some(true),
                None,
                None,
                None,
                None,
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::AdapterNotConfigurable { .. }));
    }

    #[test]
    fn configure_adapter_runtime_rejects_non_adapter() {
        let mut k = adapter_kernel();
        let err = k
            .configure_adapter_runtime(
                &PluginId::new("relux-tools-echo"),
                Some(true),
                None,
                None,
                None,
                None,
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::NotAnAdapter { .. }));
    }

    #[test]
    fn configure_adapter_runtime_is_disabled_by_default_and_clamps() {
        let mut k = adapter_kernel();
        let cfg = k
            .configure_adapter_runtime(
                &PluginId::new("relux-adapter-claude-cli"),
                None,
                None,
                Some(1),         // below MIN -> clamped up
                Some(u64::MAX),  // above MAX -> clamped down
                None,
            )
            .unwrap();
        assert!(!cfg.enabled, "CLI adapters must be disabled by default");
        assert_eq!(cfg.kind, AdapterKind::ClaudeCli);
        assert_eq!(cfg.resolved_command().as_deref(), Some("claude"));
        assert_eq!(
            cfg.timeout_seconds,
            relux_core::adapter::MIN_ADAPTER_TIMEOUT_SECONDS
        );
        assert_eq!(
            cfg.max_output_bytes,
            relux_core::adapter::MAX_ADAPTER_MAX_OUTPUT_BYTES
        );
    }

    #[test]
    fn generic_command_adapter_requires_a_command() {
        let mut k = adapter_kernel();
        // Install an unrecognized adapter plugin.
        let mut manifest = claude_adapter_manifest();
        manifest.id = PluginId::new("relux-adapter-mystery");
        manifest.capabilities.permissions =
            vec![Permission::new("adapter:relux-adapter-mystery:run").unwrap()];
        k.install_plugin(
            manifest,
            PluginSourceKind::LocalDir,
            "local".to_string(),
            "/tmp/mystery".to_string(),
            true,
        );
        let id = PluginId::new("relux-adapter-mystery");
        // No command -> invalid.
        let err = k
            .configure_adapter_runtime(&id, Some(true), None, None, None, None)
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidAdapterConfig { .. }));
        // With a command -> ok, generic kind.
        let cfg = k
            .configure_adapter_runtime(
                &id,
                Some(true),
                Some("my-agent".to_string()),
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(cfg.kind, AdapterKind::Command);
        assert_eq!(cfg.resolved_command().as_deref(), Some("my-agent"));
    }

    /// Build a claude-cli agent + an assigned task ready to run.
    fn cli_task(k: &mut KernelState) -> (AgentId, TaskId) {
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-claude-cli");
        let agent = k
            .create_agent(
                "coder",
                "Coder",
                "writes code",
                &adapter,
                &ns,
                Some("You are careful.".to_string()),
                vec![],
            )
            .unwrap();
        let task = k.create_task(
            "Summarize the repo",
            serde_json::json!({ "path": "." }),
            "founder",
            &ns,
            vec![],
        );
        k.assign_task(&task, &agent).unwrap();
        (agent, task)
    }

    #[test]
    fn execute_assigned_run_refuses_unconfigured_cli_adapter() {
        let mut k = adapter_kernel();
        let (_agent, task) = cli_task(&mut k);
        let err = k.execute_assigned_run(&task).unwrap_err();
        assert!(matches!(
            err,
            KernelError::AdapterRuntimeNotConfigured { .. }
        ));
        // The run and task are honestly marked failed (no fabricated success).
        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Failed);
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "adapter:execute" && e.result == AuditResult::Failed));
    }

    #[test]
    fn execute_assigned_run_refuses_disabled_cli_adapter() {
        let mut k = adapter_kernel();
        // Configure but leave disabled (default).
        k.configure_adapter_runtime(
            &PluginId::new("relux-adapter-claude-cli"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let (_agent, task) = cli_task(&mut k);
        let err = k.execute_assigned_run(&task).unwrap_err();
        assert!(matches!(err, KernelError::AdapterRuntimeDisabled { .. }));
        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Failed);
    }

    #[test]
    fn execute_assigned_run_spawns_enabled_cli_adapter() {
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-claude", "RAN_OK_42");
        let mut k = adapter_kernel();
        // Enable the claude adapter, but override the binary with our fake CLI.
        k.configure_adapter_runtime(
            &PluginId::new("relux-adapter-claude-cli"),
            Some(true),
            Some(fake.to_string_lossy().to_string()),
            Some(30),
            Some(4096),
            None,
        )
        .unwrap();
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Completed);
        assert_eq!(k.run(&run_id).unwrap().status, RunStatus::Completed);
        // The transcript records the spawn and the (redacted) output.
        let kinds: Vec<&str> = k.run_events(&run_id).iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"adapter_spawn"));
        assert!(kinds.contains(&"adapter_output"));
        assert!(k
            .run(&run_id)
            .unwrap()
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("RAN_OK_42"));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "adapter:execute" && e.result == AuditResult::Success));
    }

    #[test]
    fn execute_assigned_run_still_echoes_for_local_prime() {
        // A local-prime agent uses the deterministic echo path unchanged.
        let (mut k, _prime, task, run, _echo) = primed_kernel();
        let completed = k.execute_assigned_run(&task).expect("echo path ok");
        assert_eq!(completed, run);
        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Completed);
    }

    /// Write a fake CLI that prints a fixed body verbatim (no shell escaping of
    /// the body's quotes), used to emit a JSON result envelope.
    fn write_fake_json_cli(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            // `echo` prints the rest of the line literally (quotes included). The
            // JSON we emit contains no cmd metacharacters (>, <, |, &, ^).
            std::fs::write(&path, format!("@echo off\r\necho {body}\r\n")).unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            // Single-quote the body so its double quotes survive to stdout.
            std::fs::write(&path, format!("#!/bin/sh\necho '{body}'\n")).unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    /// Write a fake CLI that exits non-zero (a deterministic failure).
    fn write_failing_cli(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            std::fs::write(&path, "@echo off\r\necho boom 1>&2\r\nexit /b 3\r\n").unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\necho boom 1>&2\nexit 3\n").unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    fn enable_claude_with(k: &mut KernelState, binary: &std::path::Path) {
        k.configure_adapter_runtime(
            &PluginId::new("relux-adapter-claude-cli"),
            Some(true),
            Some(binary.to_string_lossy().to_string()),
            Some(30),
            Some(8192),
            None,
        )
        .unwrap();
    }

    #[test]
    fn cli_run_parses_structured_envelope_and_records_metrics() {
        // A CLI that emits a Claude-style JSON result envelope is parsed into an
        // honest text summary + cost/usage; the raw output is still on the
        // transcript and a real duration is recorded.
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"type":"result","is_error":false,"result":"SUMMARY_TEXT_OK","total_cost_usd":0.0125,"num_turns":3,"usage":{"output_tokens":210}}"#;
        let fake = write_fake_json_cli(dir.path(), "fake-claude-json", body);
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        let run = k.run(&run_id).unwrap();
        assert_eq!(run.status, RunStatus::Completed);
        // Summary is the envelope's `result`, not a wall of JSON.
        assert!(run.summary.as_deref().unwrap_or_default().contains("SUMMARY_TEXT_OK"));
        // Real metrics are recorded (cost parsed; duration measured).
        assert_eq!(run.cost, Some(0.0125));
        assert!(run.duration_ms.is_some());
        assert!(run.usage.is_some());
        // The adapter_output event is tagged structured and carries the raw stdout.
        let out_event = k
            .run_events(&run_id)
            .into_iter()
            .find(|e| e.kind == "adapter_output")
            .expect("adapter_output event");
        assert_eq!(out_event.payload.get("structured").and_then(|v| v.as_bool()), Some(true));
        assert!(out_event
            .payload
            .get("stdout")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("SUMMARY_TEXT_OK"));
    }

    #[test]
    fn cli_run_captures_and_persists_artifact_references() {
        // A CLI whose JSON envelope declares `artifacts` has those references
        // captured read-only onto the durable run record, sanitized, and they
        // survive a snapshot round-trip (a dashboard refresh). The adapter_output
        // event also records the count.
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"type":"result","is_error":false,"result":"edited two files","artifacts":[{"name":"main.rs","type":"file","path":"src/main.rs","summary":"added a fn"},{"type":"diff","path":"/etc/passwd"}]}"#;
        let fake = write_fake_json_cli(dir.path(), "fake-claude-arts", body);
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        let run = k.run(&run_id).unwrap();
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.artifacts.len(), 2);
        assert_eq!(run.artifacts[0].name, "main.rs");
        assert_eq!(run.artifacts[0].kind, relux_core::ArtifactKind::File);
        assert_eq!(run.artifacts[0].path.as_deref(), Some("src/main.rs"));
        assert_eq!(run.artifacts[0].source, "claude-cli");
        // The absolute path is dropped, but the reference is still captured.
        assert_eq!(run.artifacts[1].path, None);

        // The adapter_output event records the artifact count honestly.
        let out_event = k
            .run_events(&run_id)
            .into_iter()
            .find(|e| e.kind == "adapter_output")
            .expect("adapter_output event");
        assert_eq!(out_event.payload.get("artifacts").and_then(|v| v.as_u64()), Some(2));

        // Survives a snapshot round-trip (durable across a refresh / restart).
        let restored = KernelState::from_snapshot(k.snapshot());
        let restored_run = restored.run(&run_id).unwrap();
        assert_eq!(restored_run.artifacts.len(), 2);
        assert_eq!(restored_run.artifacts[0].path.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn cli_run_without_artifacts_records_none() {
        // The honest empty state: an envelope with no `artifacts` yields an empty
        // set (never a fabricated one).
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"type":"result","is_error":false,"result":"no files changed"}"#;
        let fake = write_fake_json_cli(dir.path(), "fake-claude-noart", body);
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");
        assert!(k.run(&run_id).unwrap().artifacts.is_empty());
    }

    // ── Proposed-change review/apply (master plan §15 diff/apply model) ─────

    /// Enable the claude adapter with a controlled workspace root (working_dir).
    fn enable_claude_with_workdir(k: &mut KernelState, binary: &std::path::Path, workdir: &str) {
        k.configure_adapter_runtime(
            &PluginId::new("relux-adapter-claude-cli"),
            Some(true),
            Some(binary.to_string_lossy().to_string()),
            Some(30),
            Some(8192),
            Some(workdir.to_string()),
        )
        .unwrap();
    }

    /// Build a kernel + a completed run carrying one proposed change, with the
    /// claude adapter pointed at `workdir`. The change starts in `Proposed`.
    fn kernel_with_proposed_change(
        workdir: Option<&str>,
        baseline: Option<String>,
        path: &str,
        new_content: &str,
    ) -> (KernelState, RunId) {
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-claude", "ok");
        // Leak the tempdir so the fake binary path stays valid for the test.
        std::mem::forget(dir);
        let mut k = adapter_kernel();
        k.configure_adapter_runtime(
            &PluginId::new("relux-adapter-claude-cli"),
            Some(true),
            Some(fake.to_string_lossy().to_string()),
            Some(30),
            Some(8192),
            workdir.map(|w| w.to_string()),
        )
        .unwrap();
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.start_run(&task).unwrap();
        let change = relux_core::ProposedChange {
            path: path.to_string(),
            action: relux_core::ProposedChangeAction::Replace,
            dest_path: None,
            new_content: new_content.to_string(),
            baseline_sha256: baseline,
            new_sha256: relux_core::sha256_hex(new_content.as_bytes()),
            bytes: new_content.len() as u64,
            source: "claude-cli".to_string(),
            status: relux_core::ProposedChangeStatus::Proposed,
            review_note: None,
            refused_reason: None,
            applied_at: None,
        };
        k.runs.get_mut(&run_id).unwrap().proposed_changes = vec![change];
        (k, run_id)
    }

    #[test]
    fn apply_to_workspace_writes_when_baseline_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "old").unwrap();
        let baseline = relux_core::sha256_hex(b"old");
        let n = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "out.txt",
            relux_core::ProposedChangeAction::Replace,
            Some(&baseline),
            None,
            "new content",
        )
        .expect("apply ok");
        assert_eq!(n, "new content".len() as u64);
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "new content");
    }

    #[test]
    fn apply_to_workspace_refuses_on_baseline_conflict_and_leaves_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "DIFFERENT").unwrap();
        let baseline = relux_core::sha256_hex(b"old");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "out.txt",
            relux_core::ProposedChangeAction::Replace,
            Some(&baseline),
            None,
            "new content",
        )
        .unwrap_err();
        assert!(err.conflict, "baseline mismatch is a conflict");
        // The file is untouched.
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "DIFFERENT");
    }

    #[test]
    fn apply_to_workspace_refuses_missing_target() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = relux_core::sha256_hex(b"old");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "absent.txt",
            relux_core::ProposedChangeAction::Replace,
            Some(&baseline),
            None,
            "x",
        )
        .unwrap_err();
        // A missing target (a replace needs an existing baseline file) is a conflict.
        assert!(err.conflict);
    }

    #[test]
    fn apply_to_workspace_refuses_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        // `..` is rejected by the sanitizer before any filesystem access.
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "../escape.txt",
            relux_core::ProposedChangeAction::Replace,
            Some("x"),
            None,
            "y",
        )
        .unwrap_err();
        assert!(!err.conflict);
        assert!(err.reason.contains("unsafe") || err.reason.contains("escape"));
    }

    #[test]
    fn create_to_workspace_writes_new_file_and_makes_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // The parent dir does not exist yet; a create makes it.
        let n = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "src/new/mod.rs",
            relux_core::ProposedChangeAction::Create,
            None,
            None,
            "pub fn hi() {}\n",
        )
        .expect("create ok");
        assert_eq!(n, "pub fn hi() {}\n".len() as u64);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src").join("new").join("mod.rs")).unwrap(),
            "pub fn hi() {}\n"
        );
        // No stray temp file left behind in the new parent dir.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("src").join("new"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".relux-"))
            .collect();
        assert!(leftovers.is_empty(), "no temp file should remain");
    }

    #[test]
    fn create_to_workspace_refuses_existing_file_as_conflict() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("exists.txt"), "DO NOT TOUCH").unwrap();
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "exists.txt",
            relux_core::ProposedChangeAction::Create,
            None,
            None,
            "new content",
        )
        .unwrap_err();
        // A create over an existing path is a conflict, and the file is untouched.
        assert!(err.conflict, "existing target on create is a conflict");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("exists.txt")).unwrap(),
            "DO NOT TOUCH"
        );
    }

    #[test]
    fn create_to_workspace_refuses_excluded_path() {
        let dir = tempfile::tempdir().unwrap();
        // A create still goes through the strict path gate: a secret/vcs path is
        // refused (not a conflict — a structural refusal).
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            ".git/hooks/pre-commit",
            relux_core::ProposedChangeAction::Create,
            None,
            None,
            "#!/bin/sh\n",
        )
        .unwrap_err();
        assert!(!err.conflict);
        assert!(err.reason.contains("unsafe") || err.reason.contains("excluded"));
    }

    // ── rename/move (master plan §15 diff/apply model) ─────────────────────

    #[test]
    fn rename_to_workspace_moves_file_when_baseline_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "keep me").unwrap();
        let baseline = relux_core::sha256_hex(b"keep me");
        let n = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "old.txt",
            relux_core::ProposedChangeAction::Rename,
            Some(&baseline),
            Some("new.txt"),
            "", // a rename carries no new content
        )
        .expect("rename ok");
        // The reported bytes are the moved file's size.
        assert_eq!(n, "keep me".len() as u64);
        // The source is gone; the destination has the original content intact.
        assert!(!dir.path().join("old.txt").exists(), "source must be moved away");
        assert_eq!(std::fs::read_to_string(dir.path().join("new.txt")).unwrap(), "keep me");
    }

    #[test]
    fn rename_to_workspace_makes_dest_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "x").unwrap();
        let baseline = relux_core::sha256_hex(b"x");
        apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "old.txt",
            relux_core::ProposedChangeAction::Rename,
            Some(&baseline),
            Some("moved/into/here.txt"),
            "",
        )
        .expect("rename ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("moved").join("into").join("here.txt")).unwrap(),
            "x"
        );
    }

    #[test]
    fn rename_to_workspace_refuses_when_dest_exists_as_conflict() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "src").unwrap();
        std::fs::write(dir.path().join("taken.txt"), "DO NOT TOUCH").unwrap();
        let baseline = relux_core::sha256_hex(b"src");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "old.txt",
            relux_core::ProposedChangeAction::Rename,
            Some(&baseline),
            Some("taken.txt"),
            "",
        )
        .unwrap_err();
        // An existing destination is a conflict; nothing is moved.
        assert!(err.conflict, "existing destination is a conflict");
        assert_eq!(std::fs::read_to_string(dir.path().join("old.txt")).unwrap(), "src");
        assert_eq!(std::fs::read_to_string(dir.path().join("taken.txt")).unwrap(), "DO NOT TOUCH");
    }

    #[test]
    fn rename_to_workspace_refuses_on_baseline_conflict_and_leaves_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "CHANGED ON DISK").unwrap();
        let baseline = relux_core::sha256_hex(b"what the agent saw");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "old.txt",
            relux_core::ProposedChangeAction::Rename,
            Some(&baseline),
            Some("new.txt"),
            "",
        )
        .unwrap_err();
        assert!(err.conflict, "source baseline mismatch is a conflict");
        // The source is untouched and the destination was never created.
        assert_eq!(std::fs::read_to_string(dir.path().join("old.txt")).unwrap(), "CHANGED ON DISK");
        assert!(!dir.path().join("new.txt").exists());
    }

    #[test]
    fn rename_to_workspace_refuses_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = relux_core::sha256_hex(b"x");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "absent.txt",
            relux_core::ProposedChangeAction::Rename,
            Some(&baseline),
            Some("new.txt"),
            "",
        )
        .unwrap_err();
        // A rename needs an existing source file (a missing one is a conflict).
        assert!(err.conflict);
    }

    #[test]
    fn rename_to_workspace_refuses_unsafe_destination() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "x").unwrap();
        let baseline = relux_core::sha256_hex(b"x");
        // A `..` destination escapes the root: a structural refusal, source intact.
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "old.txt",
            relux_core::ProposedChangeAction::Rename,
            Some(&baseline),
            Some("../escape.txt"),
            "",
        )
        .unwrap_err();
        assert!(!err.conflict);
        assert!(err.reason.contains("unsafe") || err.reason.contains("escape"));
        assert_eq!(std::fs::read_to_string(dir.path().join("old.txt")).unwrap(), "x");
    }

    #[test]
    fn rename_to_workspace_refuses_same_source_and_dest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let baseline = relux_core::sha256_hex(b"x");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "a.txt",
            relux_core::ProposedChangeAction::Rename,
            Some(&baseline),
            Some("a.txt"),
            "",
        )
        .unwrap_err();
        assert!(!err.conflict);
        assert!(err.reason.contains("same path"));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "x");
    }

    // ── delete (master plan §15 diff/apply model) ──────────────────────────

    #[test]
    fn delete_from_workspace_removes_file_when_baseline_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gone.txt"), "remove me").unwrap();
        let baseline = relux_core::sha256_hex(b"remove me");
        let n = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "gone.txt",
            relux_core::ProposedChangeAction::Delete,
            Some(&baseline),
            None,
            "", // a delete carries no new content
        )
        .expect("delete ok");
        // The reported bytes are the removed file's size; the file is gone.
        assert_eq!(n, "remove me".len() as u64);
        assert!(!dir.path().join("gone.txt").exists(), "target must be removed");
    }

    #[test]
    fn delete_from_workspace_refuses_on_baseline_conflict_and_leaves_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "CHANGED ON DISK").unwrap();
        let baseline = relux_core::sha256_hex(b"what the agent saw");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "keep.txt",
            relux_core::ProposedChangeAction::Delete,
            Some(&baseline),
            None,
            "",
        )
        .unwrap_err();
        assert!(err.conflict, "baseline mismatch is a conflict");
        // The file is untouched (a delete never removes a file that moved on us).
        assert_eq!(std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(), "CHANGED ON DISK");
    }

    #[test]
    fn delete_from_workspace_refuses_missing_target() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = relux_core::sha256_hex(b"x");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "absent.txt",
            relux_core::ProposedChangeAction::Delete,
            Some(&baseline),
            None,
            "",
        )
        .unwrap_err();
        // A delete needs an existing target file (a missing one is a conflict).
        assert!(err.conflict);
    }

    #[test]
    fn delete_from_workspace_refuses_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("adir")).unwrap();
        let baseline = relux_core::sha256_hex(b"x");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "adir",
            relux_core::ProposedChangeAction::Delete,
            Some(&baseline),
            None,
            "",
        )
        .unwrap_err();
        // A directory is not a regular file: a structural refusal, dir left intact.
        assert!(!err.conflict);
        assert!(err.reason.contains("not a regular file"));
        assert!(dir.path().join("adir").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn delete_from_workspace_refuses_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real.txt"), "real").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("link.txt"))
            .unwrap();
        let baseline = relux_core::sha256_hex(b"real");
        let err = apply_change_to_workspace(
            dir.path().to_str().unwrap(),
            "link.txt",
            relux_core::ProposedChangeAction::Delete,
            Some(&baseline),
            None,
            "",
        )
        .unwrap_err();
        // A symlink is rejected at path resolution (it never reaches the unlink), so
        // the link and its target both survive.
        assert!(!err.conflict);
        assert!(dir.path().join("link.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("real.txt")).unwrap(), "real");
    }

    /// Build a kernel + run carrying a single `rename` proposed change moving
    /// `path` -> `dest`, with the source `baseline`, under a controlled `workdir`.
    fn kernel_with_rename_change(
        workdir: Option<&str>,
        baseline: Option<String>,
        path: &str,
        dest: &str,
    ) -> (KernelState, RunId) {
        let (mut k, run_id) = kernel_with_proposed_change(workdir, baseline, path, "x");
        let change = &mut k.runs.get_mut(&run_id).unwrap().proposed_changes[0];
        change.action = relux_core::ProposedChangeAction::Rename;
        change.dest_path = Some(dest.to_string());
        change.new_content = String::new();
        change.bytes = 0;
        change.new_sha256 = relux_core::sha256_hex(b"");
        (k, run_id)
    }

    #[test]
    fn review_then_apply_rename_moves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.rs"), "fn old() {}\n").unwrap();
        let baseline = relux_core::sha256_hex(b"fn old() {}\n");
        let (mut k, run_id) = kernel_with_rename_change(
            Some(dir.path().to_str().unwrap()),
            Some(baseline),
            "old.rs",
            "new.rs",
        );
        // Apply before approval is refused honestly.
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApproved { .. }));
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let applied = k.apply_proposed_change(&run_id, 0).expect("rename apply ok");
        // The applied path is the destination (where the file now lives).
        assert_eq!(applied.path, "new.rs");
        assert!(!dir.path().join("old.rs").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("new.rs")).unwrap(), "fn old() {}\n");
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert_eq!(change.status, relux_core::ProposedChangeStatus::Applied);
        assert!(change.applied_at.is_some());
    }

    #[test]
    fn apply_rename_refuses_without_a_baseline_hash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.rs"), "x").unwrap();
        let (mut k, run_id) =
            kernel_with_rename_change(Some(dir.path().to_str().unwrap()), None, "old.rs", "new.rs");
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApplicable { .. }));
        // The source was never moved.
        assert!(dir.path().join("old.rs").exists());
        assert!(!dir.path().join("new.rs").exists());
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert!(change.refused_reason.as_deref().unwrap().contains("baseline"));
    }

    /// Build a kernel + run carrying a single `delete` proposed change removing
    /// `path`, with the source `baseline`, under a controlled `workdir`.
    fn kernel_with_delete_change(
        workdir: Option<&str>,
        baseline: Option<String>,
        path: &str,
    ) -> (KernelState, RunId) {
        let (mut k, run_id) = kernel_with_proposed_change(workdir, baseline, path, "x");
        let change = &mut k.runs.get_mut(&run_id).unwrap().proposed_changes[0];
        change.action = relux_core::ProposedChangeAction::Delete;
        change.dest_path = None;
        change.new_content = String::new();
        change.bytes = 0;
        change.new_sha256 = relux_core::sha256_hex(b"");
        (k, run_id)
    }

    #[test]
    fn review_then_apply_delete_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dead.rs"), "fn dead() {}\n").unwrap();
        let baseline = relux_core::sha256_hex(b"fn dead() {}\n");
        let (mut k, run_id) =
            kernel_with_delete_change(Some(dir.path().to_str().unwrap()), Some(baseline), "dead.rs");
        // Apply before approval is refused honestly.
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApproved { .. }));
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let applied = k.apply_proposed_change(&run_id, 0).expect("delete apply ok");
        // The applied path is the removed file; its size is reported.
        assert_eq!(applied.path, "dead.rs");
        assert_eq!(applied.bytes, "fn dead() {}\n".len() as u64);
        assert!(!dir.path().join("dead.rs").exists());
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert_eq!(change.status, relux_core::ProposedChangeStatus::Applied);
        assert!(change.applied_at.is_some());
    }

    #[test]
    fn apply_delete_refuses_without_a_baseline_hash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dead.rs"), "x").unwrap();
        let (mut k, run_id) =
            kernel_with_delete_change(Some(dir.path().to_str().unwrap()), None, "dead.rs");
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApplicable { .. }));
        // The file was never removed (no force in v1).
        assert!(dir.path().join("dead.rs").exists());
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert!(change.refused_reason.as_deref().unwrap().contains("baseline"));
    }

    #[test]
    fn apply_delete_on_a_changed_file_refuses_as_conflict_and_leaves_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dead.rs"), "MOVED ON US").unwrap();
        let stale = relux_core::sha256_hex(b"what the agent saw");
        let (mut k, run_id) =
            kernel_with_delete_change(Some(dir.path().to_str().unwrap()), Some(stale), "dead.rs");
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeConflict { .. }));
        // The file is untouched and the change keeps an honest reason.
        assert_eq!(std::fs::read_to_string(dir.path().join("dead.rs")).unwrap(), "MOVED ON US");
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert_eq!(change.status, relux_core::ProposedChangeStatus::Approved);
        assert!(change.refused_reason.as_deref().unwrap().contains("baseline mismatch"));
    }

    /// Build a kernel + run carrying a single `create` proposed change with no
    /// baseline, optionally under a controlled `workdir`.
    fn kernel_with_create_change(
        workdir: Option<&str>,
        path: &str,
        new_content: &str,
    ) -> (KernelState, RunId) {
        let (mut k, run_id) = kernel_with_proposed_change(workdir, None, path, new_content);
        let change = &mut k.runs.get_mut(&run_id).unwrap().proposed_changes[0];
        change.action = relux_core::ProposedChangeAction::Create;
        change.baseline_sha256 = None;
        (k, run_id)
    }

    #[test]
    fn review_then_apply_create_writes_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let (mut k, run_id) =
            kernel_with_create_change(Some(dir.path().to_str().unwrap()), "added.txt", "brand new\n");
        // Apply before approval is refused honestly.
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApproved { .. }));
        // A create needs NO baseline — approve and apply.
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let applied = k.apply_proposed_change(&run_id, 0).expect("create apply ok");
        assert_eq!(applied.path, "added.txt");
        assert_eq!(std::fs::read_to_string(dir.path().join("added.txt")).unwrap(), "brand new\n");
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert_eq!(change.status, relux_core::ProposedChangeStatus::Applied);
        assert!(change.applied_at.is_some());
    }

    #[test]
    fn apply_create_over_existing_file_refuses_as_conflict_and_leaves_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("added.txt"), "ALREADY HERE").unwrap();
        let (mut k, run_id) =
            kernel_with_create_change(Some(dir.path().to_str().unwrap()), "added.txt", "new\n");
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeConflict { .. }));
        // The pre-existing file is untouched and the change keeps an honest reason.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("added.txt")).unwrap(),
            "ALREADY HERE"
        );
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert_eq!(change.status, relux_core::ProposedChangeStatus::Approved);
        assert!(change.refused_reason.as_deref().unwrap().contains("already exists"));
    }

    #[test]
    fn review_then_apply_writes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "old").unwrap();
        let baseline = relux_core::sha256_hex(b"old");
        let (mut k, run_id) = kernel_with_proposed_change(
            Some(dir.path().to_str().unwrap()),
            Some(baseline),
            "out.txt",
            "applied!",
        );
        // Apply before approval is refused honestly.
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApproved { .. }));
        // Approve, then apply.
        let status = k.review_proposed_change(&run_id, 0, true, Some("ok")).unwrap();
        assert_eq!(status, relux_core::ProposedChangeStatus::Approved);
        let applied = k.apply_proposed_change(&run_id, 0).expect("apply ok");
        assert_eq!(applied.path, "out.txt");
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "applied!");
        // The change is now Applied with a stamp, and a transcript event + audit landed.
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert_eq!(change.status, relux_core::ProposedChangeStatus::Applied);
        assert!(change.applied_at.is_some());
        assert!(k
            .run_events(&run_id)
            .into_iter()
            .any(|e| e.kind == "proposed_change_applied"));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "proposed_change:apply" && e.result == AuditResult::Success));
        // Re-applying an Applied change is refused (no double-write).
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApproved { .. }));
    }

    #[test]
    fn apply_refuses_without_a_baseline_hash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "old").unwrap();
        let (mut k, run_id) = kernel_with_proposed_change(
            Some(dir.path().to_str().unwrap()),
            None, // no baseline
            "out.txt",
            "x",
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApplicable { .. }));
        // The honest reason is recorded on the change for the dashboard.
        assert!(k.run(&run_id).unwrap().proposed_changes[0]
            .refused_reason
            .as_deref()
            .unwrap_or_default()
            .contains("baseline"));
        // The file was never touched.
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "old");
    }

    #[test]
    fn apply_refuses_without_a_workspace_root() {
        let (mut k, run_id) = kernel_with_proposed_change(
            None, // no working_dir configured
            Some(relux_core::sha256_hex(b"old")),
            "out.txt",
            "x",
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApplicable { .. }));
        assert!(k.run(&run_id).unwrap().proposed_changes[0]
            .refused_reason
            .as_deref()
            .unwrap_or_default()
            .contains("workspace"));
    }

    #[test]
    fn rejected_change_cannot_be_applied_and_review_after_applied_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "old").unwrap();
        let (mut k, run_id) = kernel_with_proposed_change(
            Some(dir.path().to_str().unwrap()),
            Some(relux_core::sha256_hex(b"old")),
            "out.txt",
            "x",
        );
        let status = k.review_proposed_change(&run_id, 0, false, Some("no thanks")).unwrap();
        assert_eq!(status, relux_core::ProposedChangeStatus::Rejected);
        let err = k.apply_proposed_change(&run_id, 0).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeNotApproved { .. }));
    }

    #[test]
    fn cli_run_captures_proposed_changes_and_apply_writes_end_to_end() {
        // The full first-class slice: a fake CLI emits an envelope with a
        // proposed_changes full-content replacement (+ a correct baseline hash);
        // the kernel captures it onto the durable run, the operator approves, and
        // apply writes the new content into the controlled workspace root.
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("greeting.txt"), "hello").unwrap();
        let baseline = relux_core::sha256_hex(b"hello");
        let cli_dir = tempfile::tempdir().unwrap();
        let body = format!(
            r#"{{"type":"result","is_error":false,"result":"rewrote greeting","proposed_changes":[{{"path":"greeting.txt","content":"goodbye","baseline_sha256":"{baseline}"}}]}}"#
        );
        let fake = write_fake_json_cli(cli_dir.path(), "fake-claude-pc", &body);
        let mut k = adapter_kernel();
        enable_claude_with_workdir(&mut k, &fake, work.path().to_str().unwrap());
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        // Captured onto the run (still Proposed; capturing never applies).
        let change = &k.run(&run_id).unwrap().proposed_changes[0];
        assert_eq!(change.path, "greeting.txt");
        assert_eq!(change.new_content, "goodbye");
        assert_eq!(change.status, relux_core::ProposedChangeStatus::Proposed);
        // The file is untouched until an explicit apply.
        assert_eq!(std::fs::read_to_string(work.path().join("greeting.txt")).unwrap(), "hello");

        // Survives a snapshot round-trip (a dashboard refresh).
        let restored = KernelState::from_snapshot(k.snapshot());
        assert_eq!(restored.run(&run_id).unwrap().proposed_changes.len(), 1);

        // Approve + apply → the file is rewritten.
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        k.apply_proposed_change(&run_id, 0).expect("apply ok");
        assert_eq!(std::fs::read_to_string(work.path().join("greeting.txt")).unwrap(), "goodbye");
    }

    /// Build a kernel + a completed run carrying SEVERAL proposed changes, with
    /// the claude adapter pointed at `workdir`. Each change starts in `Proposed`.
    /// `changes` is `(path, baseline, new_content)`.
    fn kernel_with_proposed_changes(
        workdir: Option<&str>,
        changes: &[(&str, Option<String>, &str)],
    ) -> (KernelState, RunId) {
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-claude", "ok");
        std::mem::forget(dir);
        let mut k = adapter_kernel();
        k.configure_adapter_runtime(
            &PluginId::new("relux-adapter-claude-cli"),
            Some(true),
            Some(fake.to_string_lossy().to_string()),
            Some(30),
            Some(8192),
            workdir.map(|w| w.to_string()),
        )
        .unwrap();
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.start_run(&task).unwrap();
        let proposed: Vec<relux_core::ProposedChange> = changes
            .iter()
            .map(|(path, baseline, new_content)| relux_core::ProposedChange {
                path: path.to_string(),
                action: relux_core::ProposedChangeAction::Replace,
                dest_path: None,
                new_content: new_content.to_string(),
                baseline_sha256: baseline.clone(),
                new_sha256: relux_core::sha256_hex(new_content.as_bytes()),
                bytes: new_content.len() as u64,
                source: "claude-cli".to_string(),
                status: relux_core::ProposedChangeStatus::Proposed,
                review_note: None,
                refused_reason: None,
                applied_at: None,
            })
            .collect();
        k.runs.get_mut(&run_id).unwrap().proposed_changes = proposed;
        (k, run_id)
    }

    #[test]
    fn change_set_applies_multiple_files_atomically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "old-b").unwrap();
        let (mut k, run_id) = kernel_with_proposed_changes(
            Some(dir.path().to_str().unwrap()),
            &[
                ("a.txt", Some(relux_core::sha256_hex(b"old-a")), "new-a"),
                ("b.txt", Some(relux_core::sha256_hex(b"old-b")), "new-b"),
            ],
        );
        // A set apply before approval is refused with NOTHING written.
        let err = k.apply_proposed_change_set(&run_id, &[0, 1]).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeSetNotApplicable { .. }));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
        assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "old-b");

        // Approve both, then apply as one transaction.
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        k.review_proposed_change(&run_id, 1, true, None).unwrap();
        let applied = k.apply_proposed_change_set(&run_id, &[0, 1]).expect("apply set ok");
        assert_eq!(applied.applied.len(), 2);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "new-a");
        assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "new-b");
        // Both changes flipped to Applied with the SAME shared stamp.
        let run = k.run(&run_id).unwrap();
        assert!(run
            .proposed_changes
            .iter()
            .all(|c| c.status == relux_core::ProposedChangeStatus::Applied));
        assert_eq!(
            run.proposed_changes[0].applied_at,
            run.proposed_changes[1].applied_at
        );
        // One transcript event + one success audit for the transaction.
        assert!(k
            .run_events(&run_id)
            .into_iter()
            .any(|e| e.kind == "proposed_change_set_applied"));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "proposed_change:apply_set" && e.result == AuditResult::Success));
    }

    #[test]
    fn change_set_partial_conflict_leaves_all_files_untouched() {
        // One change has a stale baseline (the file moved under it). The whole
        // transaction must refuse with NEITHER file modified.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "DIVERGED").unwrap();
        let (mut k, run_id) = kernel_with_proposed_changes(
            Some(dir.path().to_str().unwrap()),
            &[
                ("a.txt", Some(relux_core::sha256_hex(b"old-a")), "new-a"),
                ("b.txt", Some(relux_core::sha256_hex(b"old-b")), "new-b"),
            ],
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        k.review_proposed_change(&run_id, 1, true, None).unwrap();
        let err = k.apply_proposed_change_set(&run_id, &[0, 1]).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeSetConflict { .. }));
        // NOTHING was written — not even the change with the good baseline.
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
        assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "DIVERGED");
        // Both changes stay Approved (not Applied) and each carries the reason.
        let run = k.run(&run_id).unwrap();
        assert!(run
            .proposed_changes
            .iter()
            .all(|c| c.status == relux_core::ProposedChangeStatus::Approved));
        assert!(run
            .proposed_changes
            .iter()
            .all(|c| c.refused_reason.as_deref().unwrap_or_default().contains("baseline")));
        // A failed audit was recorded; no success.
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "proposed_change:apply_set" && e.result == AuditResult::Failed));
        assert!(!k
            .audit_log()
            .iter()
            .any(|e| e.action == "proposed_change:apply_set" && e.result == AuditResult::Success));
    }

    #[test]
    fn change_set_refuses_duplicate_target_paths() {
        // Two changes targeting the same file (one via backslashes) is ambiguous;
        // the transaction refuses and writes nothing.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dup.txt"), "old").unwrap();
        let baseline = relux_core::sha256_hex(b"old");
        let (mut k, run_id) = kernel_with_proposed_changes(
            Some(dir.path().to_str().unwrap()),
            &[
                ("dup.txt", Some(baseline.clone()), "first"),
                ("dup.txt", Some(baseline), "second"),
            ],
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        k.review_proposed_change(&run_id, 1, true, None).unwrap();
        let err = k.apply_proposed_change_set(&run_id, &[0, 1]).unwrap_err();
        match err {
            KernelError::ProposedChangeSetNotApplicable { reason, .. } => {
                assert!(reason.contains("conflicting"), "reason: {reason}");
            }
            other => panic!("expected NotApplicable, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(dir.path().join("dup.txt")).unwrap(), "old");
    }

    #[test]
    fn change_set_refuses_unsafe_path_in_the_set() {
        // Validation re-checks paths at apply time; an unsafe path anywhere in the
        // set refuses the whole transaction (a safe sibling is not written).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ok.txt"), "old").unwrap();
        let baseline = relux_core::sha256_hex(b"old");
        let (mut k, run_id) = kernel_with_proposed_changes(
            Some(dir.path().to_str().unwrap()),
            &[("ok.txt", Some(baseline), "new")],
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        // Tamper the stored path to an escaping one (as if storage were corrupted)
        // to prove the apply-time re-validation gate.
        k.runs.get_mut(&run_id).unwrap().proposed_changes[0].path = "../escape.txt".to_string();
        let err = k.apply_proposed_change_set(&run_id, &[0]).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeSetNotApplicable { .. }));
    }

    #[test]
    fn change_set_refuses_without_a_baseline_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "old-b").unwrap();
        let (mut k, run_id) = kernel_with_proposed_changes(
            Some(dir.path().to_str().unwrap()),
            &[
                ("a.txt", Some(relux_core::sha256_hex(b"old-a")), "new-a"),
                ("b.txt", None, "new-b"), // no baseline → refuses whole set
            ],
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        k.review_proposed_change(&run_id, 1, true, None).unwrap();
        let err = k.apply_proposed_change_set(&run_id, &[0, 1]).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeSetNotApplicable { .. }));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
    }

    #[test]
    fn change_set_refuses_empty_and_duplicate_index_selections() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        let (mut k, run_id) = kernel_with_proposed_changes(
            Some(dir.path().to_str().unwrap()),
            &[("a.txt", Some(relux_core::sha256_hex(b"old-a")), "new-a")],
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        // Empty selection.
        assert!(matches!(
            k.apply_proposed_change_set(&run_id, &[]).unwrap_err(),
            KernelError::ProposedChangeSetNotApplicable { .. }
        ));
        // Duplicate index in the selection.
        assert!(matches!(
            k.apply_proposed_change_set(&run_id, &[0, 0]).unwrap_err(),
            KernelError::ProposedChangeSetNotApplicable { .. }
        ));
        // Unknown index.
        assert!(matches!(
            k.apply_proposed_change_set(&run_id, &[0, 9]).unwrap_err(),
            KernelError::UnknownProposedChange { .. }
        ));
        // The file was never touched by any of the refusals.
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
    }

    #[test]
    fn change_set_refuses_without_a_workspace_root() {
        let (mut k, run_id) = kernel_with_proposed_changes(
            None, // no working_dir configured
            &[("a.txt", Some(relux_core::sha256_hex(b"old-a")), "new-a")],
        );
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let err = k.apply_proposed_change_set(&run_id, &[0]).unwrap_err();
        assert!(matches!(err, KernelError::ProposedChangeSetNotApplicable { .. }));
        assert!(k.run(&run_id).unwrap().proposed_changes[0]
            .refused_reason
            .as_deref()
            .unwrap_or_default()
            .contains("workspace"));
    }

    #[test]
    fn apply_set_to_workspace_writes_all_when_baselines_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("nested").join("b.txt"), "old-b").unwrap();
        let changes = vec![
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Replace,
                path: "a.txt".to_string(),
                baseline: Some(relux_core::sha256_hex(b"old-a")),
                content: "new-a".to_string(),
            },
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Replace,
                path: "nested/b.txt".to_string(),
                baseline: Some(relux_core::sha256_hex(b"old-b")),
                content: "new-b".to_string(),
            },
        ];
        let applied =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).expect("ok");
        assert_eq!(applied.len(), 2);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "new-a");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("nested").join("b.txt")).unwrap(),
            "new-b"
        );
    }

    #[test]
    fn change_set_mixes_create_and_replace_atomically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("existing.txt"), "old").unwrap();
        let changes = vec![
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Replace,
                path: "existing.txt".to_string(),
                baseline: Some(relux_core::sha256_hex(b"old")),
                content: "rewritten".to_string(),
            },
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Create,
                path: "fresh/added.rs".to_string(),
                baseline: None,
                content: "fn added() {}\n".to_string(),
            },
        ];
        let applied =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).expect("ok");
        assert_eq!(applied.len(), 2);
        assert_eq!(std::fs::read_to_string(dir.path().join("existing.txt")).unwrap(), "rewritten");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("fresh").join("added.rs")).unwrap(),
            "fn added() {}\n"
        );
    }

    #[test]
    fn change_set_create_conflict_leaves_everything_untouched() {
        // A set with a good replace + a create whose target ALREADY exists must
        // refuse the whole transaction: neither file is written.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("collide.txt"), "I EXIST").unwrap();
        let changes = vec![
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Replace,
                path: "a.txt".to_string(),
                baseline: Some(relux_core::sha256_hex(b"old-a")),
                content: "new-a".to_string(),
            },
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Create,
                path: "collide.txt".to_string(),
                baseline: None,
                content: "should not be written".to_string(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(err.conflict, "a create over an existing file is a conflict");
        // NOTHING was written — the replace target keeps its old content and the
        // existing create-target is untouched.
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
        assert_eq!(std::fs::read_to_string(dir.path().join("collide.txt")).unwrap(), "I EXIST");
    }

    #[test]
    fn change_set_rolls_back_a_created_file_on_a_later_write_failure() {
        // Force a genuine phase-2 write failure AFTER a successful write: change A
        // creates a FILE named `sub`; change B tries to create `sub/inner.txt`,
        // whose parent `sub` is now a file, not a directory. Phase 1 validates both
        // (neither exists yet), then phase 2 writes A and fails B — so the
        // already-created `sub` must be rolled back (deleted).
        let dir = tempfile::tempdir().unwrap();
        let changes = vec![
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Create,
                path: "sub".to_string(),
                baseline: None,
                content: "i am a file".to_string(),
            },
            PlannedChange {
                dest: None,
                action: relux_core::ProposedChangeAction::Create,
                path: "sub/inner.txt".to_string(),
                baseline: None,
                content: "never written".to_string(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(!err.conflict, "a write/parent failure is a structural refusal");
        // The created file `sub` was rolled back (deleted) — no net change at all.
        assert!(!dir.path().join("sub").exists(), "rolled-back create must be gone");
        assert!(err.reason.contains("rolled back") || err.reason.contains("no files were written"));
    }

    #[test]
    fn change_set_mixes_rename_replace_and_create_atomically() {
        // One transaction: move `old.txt` -> `moved.txt`, rewrite `keep.txt`, and
        // create `fresh.txt`. All three land together.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "move me").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "old-keep").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Rename,
                path: "old.txt".to_string(),
                dest: Some("moved.txt".to_string()),
                baseline: Some(relux_core::sha256_hex(b"move me")),
                content: String::new(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Replace,
                path: "keep.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"old-keep")),
                content: "new-keep".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Create,
                path: "fresh.txt".to_string(),
                dest: None,
                baseline: None,
                content: "created".to_string(),
            },
        ];
        let applied =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).expect("ok");
        assert_eq!(applied.len(), 3);
        // The rename reports the destination path and the moved file's size.
        assert_eq!(applied[0], ("moved.txt".to_string(), "move me".len() as u64));
        assert!(!dir.path().join("old.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("moved.txt")).unwrap(), "move me");
        assert_eq!(std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(), "new-keep");
        assert_eq!(std::fs::read_to_string(dir.path().join("fresh.txt")).unwrap(), "created");
    }

    #[test]
    fn change_set_rename_dest_conflict_leaves_everything_untouched() {
        // A good replace + a rename whose destination ALREADY exists must refuse the
        // whole transaction: nothing is written or moved.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("src.txt"), "move me").unwrap();
        std::fs::write(dir.path().join("taken.txt"), "I EXIST").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Replace,
                path: "a.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"old-a")),
                content: "new-a".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Rename,
                path: "src.txt".to_string(),
                dest: Some("taken.txt".to_string()),
                baseline: Some(relux_core::sha256_hex(b"move me")),
                content: String::new(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(err.conflict, "a rename onto an existing path is a conflict");
        // NOTHING was written or moved.
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
        assert_eq!(std::fs::read_to_string(dir.path().join("src.txt")).unwrap(), "move me");
        assert_eq!(std::fs::read_to_string(dir.path().join("taken.txt")).unwrap(), "I EXIST");
    }

    #[test]
    fn change_set_refuses_a_rename_baseline_conflict_and_leaves_everything() {
        // The rename's source changed on disk since the proposal: the whole set is
        // refused before any write, so the good replace is never applied either.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("src.txt"), "DIFFERENT NOW").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Replace,
                path: "a.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"old-a")),
                content: "new-a".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Rename,
                path: "src.txt".to_string(),
                dest: Some("moved.txt".to_string()),
                baseline: Some(relux_core::sha256_hex(b"what the agent saw")),
                content: String::new(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(err.conflict, "rename source baseline mismatch is a conflict");
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
        assert_eq!(std::fs::read_to_string(dir.path().join("src.txt")).unwrap(), "DIFFERENT NOW");
        assert!(!dir.path().join("moved.txt").exists());
    }

    #[test]
    fn change_set_refuses_overlapping_rename_and_create_targets() {
        // Two changes occupy the SAME destination path (a create AND a rename both
        // target `dest.txt`): the set is refused as a conflicting target, untouched.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("src.txt"), "move me").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Create,
                path: "dest.txt".to_string(),
                dest: None,
                baseline: None,
                content: "created".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Rename,
                path: "src.txt".to_string(),
                dest: Some("dest.txt".to_string()),
                baseline: Some(relux_core::sha256_hex(b"move me")),
                content: String::new(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(!err.conflict, "an overlapping target is a structural refusal");
        assert!(err.reason.contains("conflicting target path"), "reason: {}", err.reason);
        // Nothing written or moved.
        assert!(!dir.path().join("dest.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("src.txt")).unwrap(), "move me");
    }

    #[test]
    fn change_set_refuses_renaming_a_file_another_change_also_targets() {
        // One change renames `shared.txt` away; another replaces it. They occupy the
        // same source path, so the set is refused before any write.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shared.txt"), "shared").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Rename,
                path: "shared.txt".to_string(),
                dest: Some("moved.txt".to_string()),
                baseline: Some(relux_core::sha256_hex(b"shared")),
                content: String::new(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Replace,
                path: "shared.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"shared")),
                content: "rewritten".to_string(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(!err.conflict);
        assert!(err.reason.contains("conflicting target path"), "reason: {}", err.reason);
        assert!(!dir.path().join("moved.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("shared.txt")).unwrap(), "shared");
    }

    #[test]
    fn change_set_rolls_back_a_rename_on_a_later_write_failure() {
        // A rename succeeds in phase 2, then a later create fails (its parent is a
        // file). The completed rename must be rolled back: the file moves back to its
        // source and the destination is gone, leaving no net change.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("src.txt"), "move me").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Rename,
                path: "src.txt".to_string(),
                dest: Some("moved.txt".to_string()),
                baseline: Some(relux_core::sha256_hex(b"move me")),
                content: String::new(),
            },
            // `blocker` is created as a FILE; then a create under `blocker/inner.txt`
            // fails because its parent is not a directory — forcing a rollback.
            PlannedChange {
                action: relux_core::ProposedChangeAction::Create,
                path: "blocker".to_string(),
                dest: None,
                baseline: None,
                content: "i am a file".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Create,
                path: "blocker/inner.txt".to_string(),
                dest: None,
                baseline: None,
                content: "never written".to_string(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(!err.conflict, "a phase-2 parent failure is a structural refusal");
        // The rename was rolled back: the file is back at its source, no destination.
        assert_eq!(std::fs::read_to_string(dir.path().join("src.txt")).unwrap(), "move me");
        assert!(!dir.path().join("moved.txt").exists(), "rolled-back rename dest must be gone");
        assert!(!dir.path().join("blocker").exists(), "rolled-back create must be gone");
        assert!(err.reason.contains("rolled back"));
    }

    #[test]
    fn change_set_mixes_delete_replace_and_create_atomically() {
        // One transaction: delete `drop.txt`, rewrite `keep.txt`, create `fresh.txt`.
        // All three land together.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("drop.txt"), "delete me").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "old-keep").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Delete,
                path: "drop.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"delete me")),
                content: String::new(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Replace,
                path: "keep.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"old-keep")),
                content: "new-keep".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Create,
                path: "fresh.txt".to_string(),
                dest: None,
                baseline: None,
                content: "created".to_string(),
            },
        ];
        let applied =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).expect("ok");
        assert_eq!(applied.len(), 3);
        // The delete reports its own path and the removed file's size.
        assert_eq!(applied[0], ("drop.txt".to_string(), "delete me".len() as u64));
        assert!(!dir.path().join("drop.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(), "new-keep");
        assert_eq!(std::fs::read_to_string(dir.path().join("fresh.txt")).unwrap(), "created");
    }

    #[test]
    fn change_set_refuses_delete_and_replace_of_the_same_path() {
        // One change deletes `shared.txt`; another replaces it. They occupy the same
        // path, so the set is refused before any write — nothing is removed or
        // rewritten. (A set that wants replace + delete the same path is refused.)
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shared.txt"), "shared").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Delete,
                path: "shared.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"shared")),
                content: String::new(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Replace,
                path: "shared.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"shared")),
                content: "rewritten".to_string(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(!err.conflict, "an overlapping target is a structural refusal");
        assert!(err.reason.contains("conflicting target path"), "reason: {}", err.reason);
        assert_eq!(std::fs::read_to_string(dir.path().join("shared.txt")).unwrap(), "shared");
    }

    #[test]
    fn change_set_refuses_a_delete_baseline_conflict_and_leaves_everything() {
        // The delete's target changed on disk since the proposal: the whole set is
        // refused before any write, so the good replace is never applied either.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old-a").unwrap();
        std::fs::write(dir.path().join("drop.txt"), "DIFFERENT NOW").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Replace,
                path: "a.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"old-a")),
                content: "new-a".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Delete,
                path: "drop.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"what the agent saw")),
                content: String::new(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(err.conflict, "delete baseline mismatch is a conflict");
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "old-a");
        assert_eq!(std::fs::read_to_string(dir.path().join("drop.txt")).unwrap(), "DIFFERENT NOW");
    }

    #[test]
    fn change_set_rolls_back_a_delete_on_a_later_write_failure() {
        // A delete succeeds in phase 2, then a later create fails (its parent is a
        // file). The completed delete must be rolled back: the file is recreated from
        // its captured bytes, leaving no net change.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("drop.txt"), "bring me back").unwrap();
        let changes = vec![
            PlannedChange {
                action: relux_core::ProposedChangeAction::Delete,
                path: "drop.txt".to_string(),
                dest: None,
                baseline: Some(relux_core::sha256_hex(b"bring me back")),
                content: String::new(),
            },
            // `blocker` is created as a FILE; then a create under `blocker/inner.txt`
            // fails because its parent is not a directory — forcing a rollback.
            PlannedChange {
                action: relux_core::ProposedChangeAction::Create,
                path: "blocker".to_string(),
                dest: None,
                baseline: None,
                content: "i am a file".to_string(),
            },
            PlannedChange {
                action: relux_core::ProposedChangeAction::Create,
                path: "blocker/inner.txt".to_string(),
                dest: None,
                baseline: None,
                content: "never written".to_string(),
            },
        ];
        let err =
            apply_change_set_to_workspace(dir.path().to_str().unwrap(), &changes).unwrap_err();
        assert!(!err.conflict, "a phase-2 parent failure is a structural refusal");
        // The delete was rolled back: the file is restored to its original content.
        assert_eq!(std::fs::read_to_string(dir.path().join("drop.txt")).unwrap(), "bring me back");
        assert!(!dir.path().join("blocker").exists(), "rolled-back create must be gone");
        assert!(err.reason.contains("rolled back"));
    }

    #[test]
    fn cli_run_captures_two_proposed_changes_and_set_apply_writes_both_end_to_end() {
        // The transactional slice end-to-end: a fake CLI emits an envelope with TWO
        // proposed_changes (each a full-content replacement + correct baseline);
        // the kernel captures both, the operator approves both, and ONE set apply
        // rewrites both files in the controlled workspace root.
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("one.txt"), "hello").unwrap();
        std::fs::write(work.path().join("two.txt"), "world").unwrap();
        let base_one = relux_core::sha256_hex(b"hello");
        let base_two = relux_core::sha256_hex(b"world");
        let cli_dir = tempfile::tempdir().unwrap();
        let body = format!(
            r#"{{"type":"result","is_error":false,"result":"rewrote both","proposed_changes":[{{"path":"one.txt","content":"HELLO","baseline_sha256":"{base_one}"}},{{"path":"two.txt","content":"WORLD","baseline_sha256":"{base_two}"}}]}}"#
        );
        let fake = write_fake_json_cli(cli_dir.path(), "fake-claude-set", &body);
        let mut k = adapter_kernel();
        enable_claude_with_workdir(&mut k, &fake, work.path().to_str().unwrap());
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        // Both captured, still Proposed; files untouched until apply.
        assert_eq!(k.run(&run_id).unwrap().proposed_changes.len(), 2);
        assert_eq!(std::fs::read_to_string(work.path().join("one.txt")).unwrap(), "hello");

        // Approve both, apply as one transaction.
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        k.review_proposed_change(&run_id, 1, true, None).unwrap();
        let applied = k.apply_proposed_change_set(&run_id, &[0, 1]).expect("set apply ok");
        assert_eq!(applied.applied.len(), 2);
        assert_eq!(std::fs::read_to_string(work.path().join("one.txt")).unwrap(), "HELLO");
        assert_eq!(std::fs::read_to_string(work.path().join("two.txt")).unwrap(), "WORLD");
    }

    #[test]
    fn cli_run_captures_one_create_and_one_replace_and_set_apply_writes_both_end_to_end() {
        // The create slice end-to-end: a fake CLI emits an envelope with ONE
        // `action:"create"` (a new file, no baseline) and ONE `action:"replace"`
        // (over an existing baseline). The kernel captures both, the operator
        // approves both, and ONE set apply writes the new file AND rewrites the
        // existing one.
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("keep.txt"), "old-keep").unwrap();
        let base_keep = relux_core::sha256_hex(b"old-keep");
        let cli_dir = tempfile::tempdir().unwrap();
        let body = format!(
            r#"{{"type":"result","is_error":false,"result":"added + rewrote","proposed_changes":[{{"path":"docs/new.md","action":"create","content":"brand new doc\n"}},{{"path":"keep.txt","action":"replace","content":"new-keep","baseline_sha256":"{base_keep}"}}]}}"#
        );
        let fake = write_fake_json_cli(cli_dir.path(), "fake-claude-create", &body);
        let mut k = adapter_kernel();
        enable_claude_with_workdir(&mut k, &fake, work.path().to_str().unwrap());
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        // Both captured; the create carries the create action and no baseline.
        let changes = &k.run(&run_id).unwrap().proposed_changes;
        assert_eq!(changes.len(), 2);
        let create = changes.iter().find(|c| c.path == "docs/new.md").unwrap();
        assert_eq!(create.action, relux_core::ProposedChangeAction::Create);
        assert_eq!(create.baseline_sha256, None);
        // The new file does not exist yet (capture never writes).
        assert!(!work.path().join("docs").join("new.md").exists());

        // Approve both, apply as one transaction.
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        k.review_proposed_change(&run_id, 1, true, None).unwrap();
        let applied = k.apply_proposed_change_set(&run_id, &[0, 1]).expect("set apply ok");
        assert_eq!(applied.applied.len(), 2);
        assert_eq!(
            std::fs::read_to_string(work.path().join("docs").join("new.md")).unwrap(),
            "brand new doc\n"
        );
        assert_eq!(std::fs::read_to_string(work.path().join("keep.txt")).unwrap(), "new-keep");
    }

    #[test]
    fn cli_run_captures_a_rename_and_apply_moves_the_file_end_to_end() {
        // The rename slice end-to-end: a fake CLI emits an envelope with ONE
        // `action:"rename"` (source path + a `to` destination + the source
        // baseline). The kernel captures it, the operator approves it, and apply
        // moves the file inside the controlled workspace root.
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("old_name.rs"), "fn thing() {}\n").unwrap();
        let base = relux_core::sha256_hex(b"fn thing() {}\n");
        let cli_dir = tempfile::tempdir().unwrap();
        let body = format!(
            r#"{{"type":"result","is_error":false,"result":"renamed it","proposed_changes":[{{"path":"old_name.rs","action":"rename","to":"src/new_name.rs","baseline_sha256":"{base}"}}]}}"#
        );
        let fake = write_fake_json_cli(cli_dir.path(), "fake-claude-rename", &body);
        let mut k = adapter_kernel();
        enable_claude_with_workdir(&mut k, &fake, work.path().to_str().unwrap());
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        // Captured as a rename with its destination; the file has not moved yet.
        let changes = &k.run(&run_id).unwrap().proposed_changes;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, relux_core::ProposedChangeAction::Rename);
        assert_eq!(changes[0].dest_path.as_deref(), Some("src/new_name.rs"));
        assert!(work.path().join("old_name.rs").exists());

        // Approve + apply: the file moves (and its parent dir is created).
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let applied = k.apply_proposed_change(&run_id, 0).expect("rename apply ok");
        assert_eq!(applied.path, "src/new_name.rs");
        assert!(!work.path().join("old_name.rs").exists());
        assert_eq!(
            std::fs::read_to_string(work.path().join("src").join("new_name.rs")).unwrap(),
            "fn thing() {}\n"
        );
    }

    #[test]
    fn cli_run_captures_a_delete_and_apply_removes_the_file_end_to_end() {
        // The delete slice end-to-end: a fake CLI emits an envelope with ONE
        // `action:"delete"` (target path + the source baseline). The kernel captures
        // it, the operator approves it, and apply removes the file inside the
        // controlled workspace root.
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("obsolete.rs"), "fn old() {}\n").unwrap();
        let base = relux_core::sha256_hex(b"fn old() {}\n");
        let cli_dir = tempfile::tempdir().unwrap();
        let body = format!(
            r#"{{"type":"result","is_error":false,"result":"removed it","proposed_changes":[{{"path":"obsolete.rs","action":"delete","baseline_sha256":"{base}"}}]}}"#
        );
        let fake = write_fake_json_cli(cli_dir.path(), "fake-claude-delete", &body);
        let mut k = adapter_kernel();
        enable_claude_with_workdir(&mut k, &fake, work.path().to_str().unwrap());
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        // Captured as a delete with the source baseline; the file is still present.
        let changes = &k.run(&run_id).unwrap().proposed_changes;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, relux_core::ProposedChangeAction::Delete);
        assert_eq!(changes[0].dest_path, None);
        assert_eq!(changes[0].new_content, "");
        assert!(work.path().join("obsolete.rs").exists());

        // Approve + apply: the file is removed.
        k.review_proposed_change(&run_id, 0, true, None).unwrap();
        let applied = k.apply_proposed_change(&run_id, 0).expect("delete apply ok");
        assert_eq!(applied.path, "obsolete.rs");
        assert_eq!(applied.bytes, "fn old() {}\n".len() as u64);
        assert!(!work.path().join("obsolete.rs").exists());
    }

    #[test]
    fn cli_run_envelope_error_on_clean_exit_is_an_honest_failure() {
        // Exit 0 but `is_error: true` must NOT be recorded as success.
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"type":"result","is_error":true,"result":"rate limited"}"#;
        let fake = write_fake_json_cli(dir.path(), "fake-claude-err", body);
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let err = k.execute_assigned_run(&task).unwrap_err();
        assert!(matches!(err, KernelError::AdapterExecutionFailed { .. }));
        assert_eq!(k.task(&task).unwrap().status, TaskStatus::Failed);
        let run = k.runs().into_iter().next_back().unwrap();
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.error.as_deref().unwrap_or_default().contains("reported an error"));
    }

    #[test]
    fn retry_run_creates_a_fresh_linked_run() {
        // A failed run can be retried; the retry is a brand-new run on the same
        // task, with lineage recorded. (Same failing binary -> the retry also
        // fails, which is fine: we are asserting the retry mechanics + lineage.)
        let dir = tempfile::tempdir().unwrap();
        let fake = write_failing_cli(dir.path(), "fake-fail");
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let first = k.execute_assigned_run(&task).unwrap_err();
        assert!(matches!(first, KernelError::AdapterExecutionFailed { .. }));
        let first_run = k.runs().into_iter().next_back().unwrap().id.clone();
        assert_eq!(k.run(&first_run).unwrap().status, RunStatus::Failed);

        // Retry -> a fresh run, linked back to the first, on the same task.
        let second_run = k.retry_run(&first_run).unwrap_err(); // same binary still fails
        assert!(matches!(second_run, KernelError::AdapterExecutionFailed { .. }));
        // The new run exists, differs from the first, and carries lineage.
        let newest = k.runs().into_iter().next_back().unwrap();
        assert_ne!(newest.id, first_run);
        assert_eq!(newest.retried_from.as_ref(), Some(&first_run));
        assert_eq!(newest.task_id, task);
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "run:retry"));
    }

    #[test]
    fn retry_run_refuses_a_run_that_did_not_fail() {
        // Only failed runs are retryable.
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-ok", "OK");
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("ok run");
        assert_eq!(k.run(&run_id).unwrap().status, RunStatus::Completed);
        let err = k.retry_run(&run_id).unwrap_err();
        assert!(matches!(err, KernelError::RunNotRetryable { .. }));
    }

    #[test]
    fn adapter_runtime_status_reports_local_and_cli() {
        let mut k = adapter_kernel();
        let statuses = k.adapter_runtime_status();
        // local-prime + claude-cli are both adapters.
        let local = statuses
            .iter()
            .find(|s| s.plugin_id == "relux-adapter-local-prime")
            .unwrap();
        assert_eq!(local.state, AdapterRuntimeState::LocalDeterministic);
        let claude = statuses
            .iter()
            .find(|s| s.plugin_id == "relux-adapter-claude-cli")
            .unwrap();
        // Disabled-by-default safe state before any configuration.
        assert_eq!(claude.state, AdapterRuntimeState::NeedsConfiguration);
        assert!(!claude.enabled);

        // After enabling with a missing binary, status flips to MissingBinary.
        k.configure_adapter_runtime(
            &PluginId::new("relux-adapter-claude-cli"),
            Some(true),
            Some("relux-not-a-real-binary-xyz".to_string()),
            None,
            None,
            None,
        )
        .unwrap();
        let claude = k
            .adapter_runtime_status()
            .into_iter()
            .find(|s| s.plugin_id == "relux-adapter-claude-cli")
            .unwrap();
        assert_eq!(claude.state, AdapterRuntimeState::MissingBinary);
        assert!(claude.enabled);
        assert!(!claude.available_on_path);
    }

    // --- Orchestration (multi-agent autonomy) ------------------------------

    /// A kernel with Prime plus a local-adapter `code-agent` that holds the echo
    /// permission, so a brief assigned to it can actually run the deterministic
    /// local path (mirrors a hired specialist on the safe local adapter).
    fn orchestration_kernel() -> (KernelState, PrimeContext) {
        let (mut k, ctx) = prime_chat_kernel();
        let adapter = PluginId::new("relux-adapter-local-prime");
        k.create_agent(
            "code-agent",
            "Code Agent",
            "implements code",
            &adapter,
            &ctx.namespace,
            None,
            vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
        )
        .unwrap();
        (k, ctx)
    }

    #[test]
    fn prime_orchestrate_creates_role_briefs_assigned_to_agents() {
        let (mut k, ctx) = orchestration_kernel();
        let record = k
            .prime_orchestrate(
                &ctx,
                "research the options, implement the prototype, and document it",
            )
            .unwrap();

        assert_eq!(record.steps.len(), 3);
        assert_eq!(record.status, OrchestrationStatus::Planned);
        // Implementation step routes to the code-agent specialist; research and
        // documentation have no specialist, so they fall back to Prime.
        assert_eq!(record.steps[0].agent_id.as_str(), "prime"); // research
        assert_eq!(record.steps[1].agent_id.as_str(), "code-agent"); // implement
        assert_eq!(record.steps[2].agent_id.as_str(), "prime"); // document
        // Every brief became a real, assigned (Queued) task.
        for step in &record.steps {
            let task = k.task(&step.task_id).expect("brief task exists");
            assert_eq!(task.status, TaskStatus::Queued);
            assert!(task.assigned_agent.is_some());
            assert_eq!(step.outcome, StepOutcome::Pending);
        }
        // The link is durable and auditable.
        assert_eq!(k.orchestration_count(), 1);
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "orchestration:create" && e.result == AuditResult::Success));
        // Nothing ran yet.
        assert_eq!(k.run_count(), 0);
    }

    #[test]
    fn prime_orchestrate_rejects_a_single_step_goal() {
        let (mut k, ctx) = orchestration_kernel();
        let err = k.prime_orchestrate(&ctx, "summarize the README").unwrap_err();
        assert!(matches!(err, KernelError::OrchestrationNotMultiAgent));
        assert_eq!(k.orchestration_count(), 0, "no record on a non-split goal");
    }

    #[test]
    fn run_orchestration_runs_multiple_agents_and_records_outcomes() {
        let (mut k, ctx) = orchestration_kernel();
        let record = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap();
        let id = record.id.clone();

        let batch = k.run_orchestration(&id, 25, 2).unwrap();

        assert_eq!(batch.ran, 2);
        assert_eq!(batch.completed, 2);
        assert_eq!(batch.failed, 0);
        assert_eq!(batch.blocked, 0);
        assert_eq!(batch.pending, 0);
        assert_eq!(batch.status, OrchestrationStatus::Completed);
        // Per-agent outcomes are recorded for both agents.
        assert_eq!(batch.per_agent.len(), 2);
        assert!(batch.per_agent.iter().any(|l| l.contains("code-agent")));
        assert!(batch.per_agent.iter().any(|l| l.contains("prime")));
        // The durable record links each brief to its run and is marked completed.
        let stored = k.orchestration(&id).unwrap();
        assert_eq!(stored.status, OrchestrationStatus::Completed);
        for step in &stored.steps {
            assert_eq!(step.outcome, StepOutcome::Completed);
            assert!(step.run_id.is_some(), "completed brief has a run id");
        }
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "orchestration:batch"));
    }

    #[test]
    fn run_orchestration_is_bounded_and_stops_safely() {
        let (mut k, ctx) = orchestration_kernel();
        let record = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap();
        let id = record.id.clone();

        // A capped batch runs only one brief and leaves the rest pending.
        let first = k.run_orchestration(&id, 1, 2).unwrap();
        assert_eq!(first.ran, 1);
        assert_eq!(first.pending, 1);
        assert_eq!(first.status, OrchestrationStatus::Running);
        assert!(first.next_action.contains("again"));

        // Running again continues the remaining brief and completes.
        let second = k.run_orchestration(&id, 25, 2).unwrap();
        assert_eq!(second.ran, 1);
        assert_eq!(second.status, OrchestrationStatus::Completed);

        // Running once more does nothing - no runaway, no re-running completed work.
        let third = k.run_orchestration(&id, 25, 2).unwrap();
        assert_eq!(third.ran, 0);
        assert_eq!(third.completed, 0);
        assert_eq!(third.status, OrchestrationStatus::Completed);
    }

    #[test]
    fn run_orchestration_blocks_on_a_disabled_cli_adapter() {
        let (mut k, ctx) = orchestration_kernel();
        // A research specialist on the Claude CLI adapter, whose runtime is NOT
        // enabled (the default). A brief assigned to it must be reported as blocked,
        // never faked, while the local Prime brief still completes.
        install_bundled(&mut k, claude_adapter_manifest());
        let claude = PluginId::new("relux-adapter-claude-cli");
        k.create_agent(
            "research-agent",
            "Research Agent",
            "investigates",
            &claude,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap();

        let record = k
            .prime_orchestrate(&ctx, "research the options and document the result")
            .unwrap();
        let id = record.id.clone();
        // research -> research-agent (Claude, disabled); document -> Prime (local).
        assert_eq!(record.steps[0].agent_id.as_str(), "research-agent");

        let batch = k.run_orchestration(&id, 25, 2).unwrap();
        assert_eq!(batch.blocked, 1);
        assert_eq!(batch.completed, 1);
        assert_eq!(batch.status, OrchestrationStatus::NeedsAttention);
        assert!(batch.next_action.contains("attention"));

        let stored = k.orchestration(&id).unwrap();
        let blocked_step = stored
            .steps
            .iter()
            .find(|s| s.agent_id.as_str() == "research-agent")
            .unwrap();
        assert_eq!(blocked_step.outcome, StepOutcome::Blocked);
        assert!(blocked_step.note.is_some(), "blocked brief carries a reason");
    }

    #[test]
    fn run_orchestration_runs_dependencies_before_dependents() {
        let (mut k, ctx) = orchestration_kernel();
        // implementation depends on research, so research must run (and complete)
        // in an earlier round than implementation.
        let record = k
            .prime_orchestrate(&ctx, "research the options and implement the prototype")
            .unwrap();
        let id = record.id.clone();
        assert_eq!(record.steps[1].depends_on, vec![0], "impl waits on research");

        let batch = k.run_orchestration(&id, 25, 2).unwrap();
        assert_eq!(batch.ran, 2);
        assert_eq!(batch.completed, 2);
        assert_eq!(batch.rounds, 2, "the dependent cannot share a round with its dep");
        assert_eq!(batch.status, OrchestrationStatus::Completed);

        let stored = k.orchestration(&id).unwrap();
        assert_eq!(stored.steps[0].round, Some(1), "research ran first");
        assert_eq!(stored.steps[1].round, Some(2), "implementation ran after");
    }

    #[test]
    fn run_orchestration_runs_independent_briefs_together_and_honors_the_cap() {
        let (mut k, ctx) = orchestration_kernel();
        // Two research clauses: same tier, no prerequisite between them, so they are
        // independent and may share a round.
        let record = k
            .prime_orchestrate(&ctx, "research the rust options and research the go options")
            .unwrap();
        let id = record.id.clone();
        assert!(
            record.steps.iter().all(|s| s.depends_on.is_empty()),
            "independent briefs have no dependencies (backward compatible)"
        );

        // concurrency 2 -> both run in a single round.
        let batch = k.run_orchestration(&id, 25, 2).unwrap();
        assert_eq!(batch.ran, 2);
        assert_eq!(batch.completed, 2);
        assert_eq!(batch.rounds, 1, "two independent briefs fit one round at cap 2");
        let stored = k.orchestration(&id).unwrap();
        assert_eq!(stored.steps[0].round, Some(1));
        assert_eq!(stored.steps[1].round, Some(1));

        // No brief runs twice: a second batch does nothing.
        let again = k.run_orchestration(&id, 25, 2).unwrap();
        assert_eq!(again.ran, 0);
        assert_eq!(again.rounds, 0);
    }

    #[test]
    fn run_orchestration_concurrency_cap_of_one_serializes_independent_briefs() {
        let (mut k, ctx) = orchestration_kernel();
        let record = k
            .prime_orchestrate(&ctx, "research the rust options and research the go options")
            .unwrap();
        let id = record.id.clone();

        // cap 1 (and the clamp: 0 -> 1) forces one brief per round.
        let batch = k.run_orchestration(&id, 25, 1).unwrap();
        assert_eq!(batch.concurrency, 1);
        assert_eq!(batch.ran, 2);
        assert_eq!(batch.rounds, 2, "cap 1 never runs two briefs in one round");
    }

    #[test]
    fn run_orchestration_blocks_a_brief_whose_dependency_did_not_complete() {
        let (mut k, ctx) = orchestration_kernel();
        // Research runs on a disabled Claude CLI (blocked); implementation depends
        // on research, so it must be honestly blocked, NOT run, NOT faked.
        install_bundled(&mut k, claude_adapter_manifest());
        let claude = PluginId::new("relux-adapter-claude-cli");
        k.create_agent(
            "research-agent",
            "Research Agent",
            "investigates",
            &claude,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap();

        let record = k
            .prime_orchestrate(&ctx, "research the options and implement the prototype")
            .unwrap();
        let id = record.id.clone();
        assert_eq!(record.steps[0].agent_id.as_str(), "research-agent");
        assert_eq!(record.steps[1].depends_on, vec![0]);
        let impl_task = record.steps[1].task_id.clone();

        let batch = k.run_orchestration(&id, 25, 2).unwrap();
        assert_eq!(batch.blocked, 1, "research blocked by its disabled runtime");
        assert_eq!(batch.dependency_blocked, 1, "implementation blocked by research");
        assert_eq!(batch.completed, 0);
        assert_eq!(batch.ran, 1, "the dependent was never executed");
        assert_eq!(batch.status, OrchestrationStatus::NeedsAttention);

        let stored = k.orchestration(&id).unwrap();
        assert_eq!(stored.steps[1].outcome, StepOutcome::Blocked);
        assert!(stored.steps[1]
            .note
            .as_ref()
            .unwrap()
            .contains("depends on"));
        assert!(stored.steps[1].run_id.is_none(), "a dep-blocked brief has no run");
        // The implementation task never started a run.
        assert!(
            !k.runs().iter().any(|r| r.task_id == impl_task),
            "no run was spawned for the dependency-blocked brief"
        );
    }

    /// A generic (Command-kind) adapter manifest with an arbitrary id, used to put
    /// a second independent brief on a *different* binary than the Claude adapter.
    fn command_adapter_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: PluginId::new(id),
            name: "Command Adapter".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::Adapter,
            description: "generic command adapter".to_string(),
            author: "test".to_string(),
            trust_level: TrustLevel::Official,
            capabilities: PluginCapability {
                tools: vec![],
                permissions: vec![Permission::new(format!("adapter:{id}:run")).unwrap()],
            },
            health: PluginHealth::Unknown,
        }
    }

    fn enable_adapter_with(k: &mut KernelState, plugin: &PluginId, binary: &std::path::Path) {
        k.configure_adapter_runtime(
            plugin,
            Some(true),
            Some(binary.to_string_lossy().to_string()),
            Some(60),
            Some(8192),
            None,
        )
        .unwrap();
    }

    /// Write a fake adapter that proves it ran *concurrently* with a sibling: it
    /// atomically claims a numbered slot directory under `mark_dir` (via `mkdir`,
    /// which is atomic, so two racing processes claim *distinct* slots), then waits
    /// until at least two slots exist before exiting 0. If two of these run in
    /// parallel both reach the barrier and succeed; if they ran sequentially the
    /// first would wait for a second arrival that never comes and time out (exit 1).
    /// So two successes is a deterministic proof of overlapping in-flight execution
    /// — no timing guess, and no shared-file write contention (each owns its slot).
    fn write_rendezvous_cli(
        scripts_dir: &std::path::Path,
        name: &str,
        mark_dir: &std::path::Path,
    ) -> std::path::PathBuf {
        std::fs::create_dir_all(mark_dir).unwrap();
        #[cfg(windows)]
        {
            let path = scripts_dir.join(format!("{name}.cmd"));
            let md = mark_dir.to_string_lossy().replace('/', "\\");
            // `ping -n 2` sleeps ~1s per try without needing `timeout` (which fails
            // when stdin is redirected, as the kernel does). ~30s ceiling.
            let body = format!(
                "@echo off\r\n\
                 set \"MD={md}\"\r\n\
                 set n=0\r\n\
                 :claim\r\n\
                 mkdir \"%MD%\\p%n%\" 2>nul && goto claimed\r\n\
                 set /a n+=1\r\n\
                 if %n% GEQ 50 exit /b 2\r\n\
                 goto claim\r\n\
                 :claimed\r\n\
                 set /a tries=0\r\n\
                 :loop\r\n\
                 set count=0\r\n\
                 for /d %%D in (\"%MD%\\p*\") do set /a count+=1\r\n\
                 if %count% GEQ 2 goto done\r\n\
                 set /a tries+=1\r\n\
                 if %tries% GEQ 30 exit /b 1\r\n\
                 ping -n 2 127.0.0.1 >nul\r\n\
                 goto loop\r\n\
                 :done\r\n\
                 echo RENDEZVOUS_OK\r\n\
                 exit /b 0\r\n"
            );
            std::fs::write(&path, body).unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = scripts_dir.join(name);
            let md = mark_dir.to_string_lossy().to_string();
            let body = format!(
                "#!/bin/sh\n\
                 MD='{md}'\n\
                 n=0\n\
                 while ! mkdir \"$MD/p$n\" 2>/dev/null; do\n\
                   n=$((n+1))\n\
                   [ \"$n\" -ge 50 ] && exit 2\n\
                 done\n\
                 tries=0\n\
                 while :; do\n\
                   count=$(ls -d \"$MD\"/p* 2>/dev/null | wc -l)\n\
                   [ \"$count\" -ge 2 ] && break\n\
                   tries=$((tries+1))\n\
                   [ \"$tries\" -ge 100 ] && exit 1\n\
                   sleep 0.1\n\
                 done\n\
                 echo RENDEZVOUS_OK\n\
                 exit 0\n"
            );
            std::fs::write(&path, body).unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    /// Drive ONE round through the real kernel split — prepare under the lock, run
    /// the prepared briefs on parallel OS threads (exactly as the server's
    /// `run_briefs_in_parallel` does), then finalize each back — and finalize the
    /// batch. Returns the round's result.
    fn run_one_parallel_round(
        k: &mut KernelState,
        id: &OrchestrationId,
        max: usize,
        concurrency: usize,
        round_no: u32,
    ) -> (OrchestrationBatchResult, usize) {
        let mut result = k.new_orchestration_batch_result(id, concurrency).unwrap();
        let prep = k
            .prepare_orchestration_round(id, max, concurrency, round_no, &mut result)
            .unwrap();
        let prepared_count = prep.prepared.len();
        let handles: Vec<_> = prep
            .prepared
            .into_iter()
            .map(|p| std::thread::spawn(move || p.run()))
            .collect();
        let finished: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for f in finished {
            k.finalize_prepared_brief(id, f, &mut result);
        }
        result.rounds = 1;
        k.finalize_orchestration_batch(id, &mut result).unwrap();
        (result, prepared_count)
    }

    #[test]
    fn parallel_round_runs_two_independent_briefs_concurrently() {
        // Two independent research briefs on an enabled Claude CLI adapter whose
        // binary is a rendezvous barrier: each completes only if the other was
        // running at the same time. Both completing is a deterministic proof of
        // true OS-parallel adapter execution (not pseudo-concurrency).
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("marks");
        let cli = write_rendezvous_cli(dir.path(), "rendezvous", &marks);

        let (mut k, ctx) = orchestration_kernel();
        install_bundled(&mut k, claude_adapter_manifest());
        let claude = PluginId::new("relux-adapter-claude-cli");
        k.create_agent(
            "research-agent",
            "Research Agent",
            "investigates",
            &claude,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap();
        enable_adapter_with(&mut k, &claude, &cli);

        let record = k
            .prime_orchestrate(&ctx, "research the rust options and research the go options")
            .unwrap();
        let id = record.id.clone();
        assert!(
            record.steps.iter().all(|s| s.depends_on.is_empty()),
            "both briefs are independent"
        );
        assert!(
            record.steps.iter().all(|s| s.agent_id.as_str() == "research-agent"),
            "both briefs run on the CLI adapter"
        );

        // Prepare returns BOTH briefs for off-lock parallel execution.
        let mut probe = k.new_orchestration_batch_result(&id, 2).unwrap();
        let prep = k
            .prepare_orchestration_round(&id, 2, 2, 1, &mut probe)
            .unwrap();
        assert_eq!(prep.prepared.len(), 2, "both CLI briefs prepared to spawn in parallel");
        assert_eq!(prep.ran_inline, 0, "nothing resolved inline; both go off-lock");
        // The in-flight briefs are visible mid-round: their runs started and the
        // durable steps are stamped while the outcome is still pending.
        let mid = k.orchestration(&id).unwrap();
        assert!(mid.steps.iter().all(|s| s.outcome == StepOutcome::Pending));
        assert!(mid.steps.iter().all(|s| s.run_id.is_some()), "runs started under the lock");
        assert_eq!(k.runs().iter().filter(|r| r.status == RunStatus::Running).count(), 2);

        // Run the two prepared briefs in parallel and finalize.
        let handles: Vec<_> = prep
            .prepared
            .into_iter()
            .map(|p| std::thread::spawn(move || p.run()))
            .collect();
        let finished: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for f in finished {
            k.finalize_prepared_brief(&id, f, &mut probe);
        }
        probe.rounds = 1;
        k.finalize_orchestration_batch(&id, &mut probe).unwrap();

        assert_eq!(probe.completed, 2, "both rendezvous briefs completed => they ran together");
        assert_eq!(probe.failed, 0);
        assert_eq!(probe.blocked, 0);
        let stored = k.orchestration(&id).unwrap();
        for step in &stored.steps {
            assert_eq!(step.outcome, StepOutcome::Completed);
            assert_eq!(step.round, Some(1), "both ran in the same round");
            assert!(step.run_id.is_some());
            assert!(step.finished_at.is_some());
        }
        assert_eq!(stored.status, OrchestrationStatus::Completed);
    }

    #[test]
    fn run_orchestration_runs_independent_briefs_truly_in_parallel() {
        // The synchronous engine `run_orchestration` — the one the blocking `/run`
        // API and the `prime orchestration run` CLI both call — must itself run
        // independent ready briefs as REAL concurrent OS processes, not sequentially.
        //
        // Proof is the same rendezvous barrier the job-path test uses: each fake
        // adapter completes only if a sibling was running at the same time (it claims
        // a slot, then waits for a second slot to appear before exiting 0; run
        // sequentially the first would wait for an arrival that never comes and time
        // out to a non-zero exit). So two completions is a deterministic proof that
        // `run_orchestration` overlapped the two adapter processes in one round. Run
        // sequentially this same setup would leave a failed/blocked brief.
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("marks");
        let cli = write_rendezvous_cli(dir.path(), "rendezvous", &marks);

        let (mut k, ctx) = orchestration_kernel();
        install_bundled(&mut k, claude_adapter_manifest());
        let claude = PluginId::new("relux-adapter-claude-cli");
        k.create_agent(
            "research-agent",
            "Research Agent",
            "investigates",
            &claude,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap();
        enable_adapter_with(&mut k, &claude, &cli);

        let record = k
            .prime_orchestrate(&ctx, "research the rust options and research the go options")
            .unwrap();
        let id = record.id.clone();
        assert!(
            record.steps.iter().all(|s| s.depends_on.is_empty()),
            "both briefs are independent"
        );

        // The single synchronous entry point both the API and the CLI use. cap 2 =>
        // both briefs ready in round 1 run their adapter processes concurrently.
        let batch = k.run_orchestration(&id, 25, 2).unwrap();

        assert_eq!(batch.ran, 2);
        assert_eq!(
            batch.completed, 2,
            "both rendezvous briefs completed => run_orchestration overlapped them"
        );
        assert_eq!(batch.failed, 0);
        assert_eq!(batch.blocked, 0);
        assert_eq!(batch.rounds, 1, "both fit one round at cap 2");
        assert_eq!(batch.status, OrchestrationStatus::Completed);
        let stored = k.orchestration(&id).unwrap();
        for step in &stored.steps {
            assert_eq!(step.outcome, StepOutcome::Completed);
            assert_eq!(step.round, Some(1), "both ran in the same round");
            assert!(step.run_id.is_some());
            assert!(step.finished_at.is_some());
        }
    }

    #[test]
    fn parallel_round_isolates_a_failure_and_merges_safely() {
        // Two independent briefs in one parallel round on DIFFERENT binaries: one OK
        // (Claude adapter), one failing (a generic command adapter). The failure of
        // one must not corrupt the other — the OK brief completes, the failing one
        // fails, and the merge tallies both honestly.
        let dir = tempfile::tempdir().unwrap();
        let ok = write_fake_cli(dir.path(), "ok-cli", "RESEARCH_OK");
        let bad = write_failing_cli(dir.path(), "bad-cli");

        let (mut k, ctx) = orchestration_kernel();
        install_bundled(&mut k, claude_adapter_manifest());
        let command = PluginId::new("relux-adapter-command-test");
        install_bundled(&mut k, command_adapter_manifest(command.as_str()));
        let claude = PluginId::new("relux-adapter-claude-cli");
        k.create_agent(
            "research-agent",
            "Research Agent",
            "investigates",
            &claude,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap();
        k.create_agent(
            "ops-agent",
            "Ops Agent",
            "ships releases",
            &command,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap();
        enable_adapter_with(&mut k, &claude, &ok);
        enable_adapter_with(&mut k, &command, &bad);

        // research (no prereq) + operations (no prereq) => two independent briefs.
        let record = k
            .prime_orchestrate(&ctx, "research the options and deploy the release")
            .unwrap();
        let id = record.id.clone();
        assert_eq!(record.steps[0].agent_id.as_str(), "research-agent");
        assert_eq!(record.steps[1].agent_id.as_str(), "ops-agent");
        assert!(record.steps.iter().all(|s| s.depends_on.is_empty()));

        let (result, prepared) = run_one_parallel_round(&mut k, &id, 2, 2, 1);
        assert_eq!(prepared, 2, "both briefs ran together off-lock");
        assert_eq!(result.completed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.status, OrchestrationStatus::NeedsAttention);

        let stored = k.orchestration(&id).unwrap();
        let research = &stored.steps[0];
        let ops = &stored.steps[1];
        assert_eq!(research.outcome, StepOutcome::Completed, "OK brief unaffected by the sibling failure");
        assert!(research.run_id.is_some());
        assert_eq!(ops.outcome, StepOutcome::Failed, "failing brief recorded honestly");
        assert!(ops.note.is_some());
        // The two briefs have distinct runs/tasks — no cross-contamination.
        assert_ne!(research.task_id, ops.task_id);
        assert_ne!(research.run_id, ops.run_id);
        // The completed brief's task really completed; the failed brief's task failed.
        assert_eq!(k.task(&research.task_id).unwrap().status, TaskStatus::Completed);
        assert_eq!(k.task(&ops.task_id).unwrap().status, TaskStatus::Failed);
    }

    #[test]
    fn parallel_prepare_preserves_dependencies_across_rounds() {
        // A dependent brief must not be prepared while its dependency is pending: the
        // first parallel round prepares only the independent research brief; the
        // dependent implementation brief runs only in a later round, after research
        // has completed. Dependency gating survives the prepare/finalize split.
        let dir = tempfile::tempdir().unwrap();
        let ok = write_fake_cli(dir.path(), "ok-cli", "OK");

        let (mut k, ctx) = orchestration_kernel();
        install_bundled(&mut k, claude_adapter_manifest());
        let claude = PluginId::new("relux-adapter-claude-cli");
        k.create_agent(
            "research-agent",
            "Research Agent",
            "investigates",
            &claude,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap();
        enable_adapter_with(&mut k, &claude, &ok);

        let record = k
            .prime_orchestrate(&ctx, "research the options and implement the prototype")
            .unwrap();
        let id = record.id.clone();
        assert_eq!(record.steps[1].depends_on, vec![0], "impl waits on research");
        let impl_task = record.steps[1].task_id.clone();

        // Round 1: only the independent research brief is ready, so only it is
        // prepared/run. The dependent implementation brief is NOT touched.
        let mut probe = k.new_orchestration_batch_result(&id, 2).unwrap();
        let prep = k
            .prepare_orchestration_round(&id, 2, 2, 1, &mut probe)
            .unwrap();
        assert_eq!(prep.prepared.len(), 1, "only research is ready in round 1");
        assert_eq!(prep.prepared[0].task_id(), &record.steps[0].task_id);
        assert!(
            !k.runs().iter().any(|r| r.task_id == impl_task),
            "the dependent brief never started a run while its dep was pending"
        );

        // Finish round 1, then run the whole batch to completion in order.
        let handles: Vec<_> = prep
            .prepared
            .into_iter()
            .map(|p| std::thread::spawn(move || p.run()))
            .collect();
        let finished: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for f in finished {
            k.finalize_prepared_brief(&id, f, &mut probe);
        }
        probe.rounds = 1;
        k.finalize_orchestration_batch(&id, &mut probe).unwrap();

        // Round 2: research is complete, so implementation becomes ready and runs.
        // (It is assigned to the local-prime code-agent, so it resolves inline under
        // the lock rather than as an off-lock spawn — what matters is that it only
        // runs now, never in round 1 while its dependency was pending.)
        let (r2, _prepared2) = run_one_parallel_round(&mut k, &id, 2, 2, 2);
        assert_eq!(r2.ran, 1, "only implementation ran in round 2");
        assert_eq!(r2.completed, 1);

        let stored = k.orchestration(&id).unwrap();
        assert_eq!(stored.steps[0].round, Some(1), "research ran first");
        assert_eq!(stored.steps[1].round, Some(2), "implementation ran after");
        assert_eq!(stored.status, OrchestrationStatus::Completed);
    }

    #[test]
    fn orchestration_persists_across_snapshot() {
        let (mut k, ctx) = orchestration_kernel();
        let record = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap();
        let id = record.id.clone();

        let restored = KernelState::from_snapshot(k.snapshot());
        let stored = restored.orchestration(&id).expect("orchestration survived");
        assert_eq!(stored.steps.len(), 2);
        assert_eq!(stored.goal, record.goal);

        // The counter resumes so the next orchestration id does not collide.
        let mut restored = restored;
        let next = restored
            .prime_orchestrate(&ctx, "research the options and test the result")
            .unwrap();
        assert_ne!(next.id, id);
    }
}
