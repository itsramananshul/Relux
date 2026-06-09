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
    clamp_runtime_timeout, validate_loopback_url, Agent, AgentId, Approval, ApprovalId,
    ApprovalStatus, AuditEvent, AuditResult, InstalledPlugin, Namespace, NamespaceId, Permission,
    PluginId, PluginManifest, PluginSourceKind, PrimeAction, PrimeAutonomyConfig,
    PrimeAutonomyTickResult, PrimeContext, PrimeDisposition, PrimePlan, PrimeTurn, RiskLevel,
    RuntimeKind, Run, RunId, RunStatus, StateSummary, Task, TaskBrief, TaskId, TaskStatus,
    ToolDescriptor, ToolExecutability, ToolInvocationResult, ToolRuntimeConfig,
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
        state.run_events = snapshot.run_events;
        state.audit_log = snapshot.audit_events;
        state.clock = Clock::from_secs(snapshot.counters.clock_secs);
        state.next_task = snapshot.counters.next_task;
        state.next_run = snapshot.counters.next_run;
        state.next_approval = snapshot.counters.next_approval;
        state.next_audit = snapshot.counters.next_audit;
        state.next_event = snapshot.counters.next_event;
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

        match plan {
            PrimePlan::Reply { text } => Ok(PrimeTurn {
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
            }),
            PrimePlan::Clarify { text } => Ok(PrimeTurn {
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
                    created_agent: None,
                    approval: Some(approval),
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
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
                    created_agent: None,
                    approval: None,
                    invoked_tool: None,
                    tool_output: None,
                    tool_error: None,
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
                })
            }
            PrimeAction::DiscoverTools => {
                let tools = self.discover_tools(Some(&ctx.agent));
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
                })
            }
            PrimeAction::InvokeTool {
                plugin_id,
                tool_name,
                input_json,
            } => self.prime_invoke_tool(ctx, intent, &action, &text, plugin_id, tool_name, input_json),
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
    fn execute_cli_run(
        &mut self,
        task_id: &TaskId,
        adapter: &PluginId,
    ) -> Result<RunId, KernelError> {
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
        if crate::adapter::find_on_path(&binary).is_none() {
            let err = KernelError::AdapterBinaryMissing {
                plugin: adapter.to_string(),
                binary: binary.clone(),
            };
            self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string());
            return Err(err);
        }

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
            program: binary.clone(),
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

        // 4. Run the process (the one place the kernel touches a real CLI).
        match crate::adapter::run_adapter_command(&spec) {
            Ok(outcome) if outcome.success => {
                let summary = render_adapter_summary(&binary, &outcome);
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
                    }),
                );
                self.complete_run(&run_id, &summary)?;
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
                        binary, config.timeout_seconds
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
                    }),
                );
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

/// Render a concise, already-redacted run summary from an adapter outcome. The
/// stdout snippet is bounded so a long transcript never bloats the summary.
fn render_adapter_summary(binary: &str, outcome: &crate::adapter::AdapterRunOutcome) -> String {
    let mut s = format!("adapter '{binary}' completed (exit 0)");
    let stdout = outcome.stdout.trim();
    if !stdout.is_empty() {
        let snippet: String = stdout.chars().take(280).collect();
        s.push_str(": ");
        s.push_str(&snippet);
        if outcome.stdout_truncated || stdout.chars().count() > 280 {
            s.push_str(" …");
        }
    }
    s
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
        PrimeAction::DiscoverTools => "list the installed tools".to_string(),
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
        // The two installed built-in tools are listed, with honest status.
        assert!(turn.reply.contains("relux-tools-echo/echo.say"), "got: {}", turn.reply);
        assert!(
            turn.reply.contains("relux-tools-status/status.summary"),
            "got: {}",
            turn.reply
        );
        assert!(turn.reply.contains("ready"), "ready tools marked: {}", turn.reply);
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
}
