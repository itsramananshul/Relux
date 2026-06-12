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
    approval_blocks_direct_invocation, clamp_runtime_timeout, plan_orchestration,
    validate_loopback_url, Agent, AgentId, Approval, ApprovalId, ApprovalStatus, AuditEvent,
    AuditResult, InstalledPlugin, Namespace, NamespaceId, Orchestration, OrchestrationBatchResult,
    OrchestrationId, OrchestrationStatus, OrchestrationStep, Permission, PersistentGrant, PluginId,
    PluginManifest,
    PluginSourceKind, PrimeAction, PrimeAutonomyConfig, PrimeAutonomyTickResult, PrimeContext,
    classify_failure, PrimeDisposition, PrimePlan, PrimeTurn, RiskLevel, RunFailureClass, RunId,
    RunRetryState, RunStatus, RuntimeKind, Run, StateSummary, StepOutcome, Task, TaskBrief, TaskId,
    TaskStatus, ToolDefinition, ToolDescriptor, ToolExecutability, ToolInvocationResult,
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
    /// Next persistent-grant id. Defaulted so snapshots from before allow-always
    /// grants load cleanly (grants start at 0).
    #[serde(default)]
    pub next_grant: u64,
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
    /// Operator-curated MCP server registrations, sorted by server id. Defaulted so
    /// older snapshots (which never wrote it) load cleanly.
    #[serde(default)]
    pub mcp_servers: Vec<relux_core::McpServerConfig>,
    pub namespaces: Vec<Namespace>,
    pub agents: Vec<Agent>,
    pub tasks: Vec<Task>,
    pub runs: Vec<Run>,
    pub approvals: Vec<Approval>,
    /// Per-tool-call approval bindings, sorted by approval id. Defaulted so older
    /// snapshots (which never wrote it) load cleanly.
    #[serde(default)]
    pub pending_tool_invocations: Vec<PendingToolInvocation>,
    /// Persistent allow-always grants, sorted by id. Defaulted so older snapshots
    /// (which never wrote it) load cleanly.
    #[serde(default)]
    pub persistent_grants: Vec<PersistentGrant>,
    /// Run transcripts, in emission order.
    pub run_events: Vec<RunEvent>,
    /// Bounded, redacted per-run log tails, sorted by run id. Defaulted so older
    /// snapshots (which never wrote it) load cleanly.
    #[serde(default)]
    pub run_logs: Vec<relux_core::RunLog>,
    /// The append-only audit log, in emission order.
    pub audit_events: Vec<AuditEvent>,
    pub prime_autonomy_config: PrimeAutonomyConfig,
    /// Durable Prime orchestrations (goal -> briefs -> agents -> runs), sorted by
    /// id. Defaulted so older snapshots load cleanly.
    #[serde(default)]
    pub orchestrations: Vec<Orchestration>,
    /// Multi-turn clarification memory, one entry per conversation key, sorted by
    /// key. Defaulted so older snapshots (which never wrote it) load cleanly.
    #[serde(default)]
    pub pending_clarifications: Vec<PendingClarificationEntry>,
    /// Bounded conversation history, one entry per conversation key, sorted by key.
    /// Defaulted so older snapshots (which never wrote it) load cleanly
    /// (`docs/prime-processing-audit.md` "Bounded conversation memory").
    #[serde(default)]
    pub conversation_histories: Vec<ConversationHistoryEntry>,
    /// Rolling compacted conversation summaries, one entry per conversation key, sorted by key.
    /// Defaulted so older snapshots (which never wrote it) load cleanly
    /// (`docs/prime-processing-audit.md` "Bounded conversation-memory compaction").
    #[serde(default)]
    pub conversation_summaries: Vec<ConversationSummaryEntry>,
    pub counters: KernelCounters,
}

/// The hard cap on how many distinct conversations' pending clarifications are kept at
/// once, so the multi-turn memory stays small regardless of how many actors talk to
/// Prime. When full, inserting a new conversation's record evicts the oldest one.
pub const MAX_PENDING_CLARIFICATIONS: usize = 32;

/// One persisted multi-turn clarification record, paired with the conversation key it
/// belongs to (`namespace::actor`). A flat, serializable export of one entry of the
/// kernel's `pending_clarifications` map (`docs/prime-processing-audit.md`
/// "Multi-turn clarify memory").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingClarificationEntry {
    /// The conversation key (`namespace::actor`) this pending clarification belongs to.
    pub key: String,
    /// The bounded pending-clarification record.
    pub pending: relux_core::PendingClarification,
}

/// One persisted conversation's bounded turn history, paired with the conversation key it
/// belongs to (`namespace::actor`). A flat, serializable export of one entry of the kernel's
/// `conversation_histories` map (`docs/prime-processing-audit.md` "Bounded conversation
/// memory"; see [`crate::prime_history`]). The turns are already secret-redacted + bounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationHistoryEntry {
    /// The conversation key (`namespace::actor`) this history belongs to.
    pub key: String,
    /// The bounded, most-recent-first-evicted list of recorded turns.
    pub turns: Vec<relux_core::ConversationTurn>,
}

/// One persisted conversation's rolling compacted summary, paired with the conversation key it
/// belongs to (`namespace::actor`). A flat, serializable export of one entry of the kernel's
/// `conversation_summaries` map (`docs/prime-processing-audit.md` "Bounded conversation-memory
/// compaction"; see [`crate::prime_history`]). The summary is already bounded + secret-redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSummaryEntry {
    /// The conversation key (`namespace::actor`) this summary belongs to.
    pub key: String,
    /// The bounded, deterministic rolling summary of older (evicted) turns.
    pub summary: relux_core::ConversationSummary,
}

/// The hard cap on the serialized size of a single tool invocation's arguments
/// that a per-call approval may carry. Matches the loopback runtime's request
/// body cap ([`crate::runtime::MAX_REQUEST_BODY_BYTES`]) so an approval can never
/// bind args the runtime would itself refuse to send.
pub const MAX_TOOL_INVOCATION_ARGS_BYTES: usize = crate::runtime::MAX_REQUEST_BODY_BYTES;

/// A pending per-tool-call approval binding (`docs/RELUX_MASTER_PLAN.md` §7.4
/// per-call approval, `docs/reference-driven-development.md` "per-tool-call
/// approval"). One of these is created alongside a generic [`Approval`] when an
/// operator requests approval to invoke a specific non-low-risk configured tool.
///
/// It binds the approval to the EXACT invocation — plugin id, tool name, the
/// permission subject (agent), and a frozen snapshot of the arguments plus their
/// SHA-256 — so an approved call can never be modified before it runs. It is
/// consumed once: after a single execution attempt (success OR runtime failure)
/// `consumed` is set and the bound call can never run again without a fresh
/// approval. This mirrors openclaw's consume-once exec-approval handoff
/// (`reference/openclaw-main/src/agents/bash-tools.exec-approval-followup-state.ts`
/// `consumeExecApprovalFollowupRuntimeHandoff`: a record keyed by an approval id,
/// matched on every bound field, deleted after a single use).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingToolInvocation {
    /// The generic [`Approval`] this binding belongs to.
    pub approval_id: ApprovalId,
    pub plugin_id: PluginId,
    pub tool_name: String,
    /// The permission subject the invocation runs as (validated to hold the
    /// tool's permission both when requested and again before execution).
    pub agent_id: AgentId,
    /// The permission checked for this invocation (`tool:<plugin>:<verb>`).
    pub permission: String,
    /// The frozen arguments snapshot. Executed verbatim — never re-supplied by
    /// the client at execute time, so an approved call cannot be modified.
    pub input: serde_json::Value,
    /// Lowercase hex SHA-256 of the canonical args bytes, re-checked before
    /// execution (defense in depth against a tampered persisted snapshot).
    pub args_sha256: String,
    /// A bounded, secret-redacted preview of the args for the Approvals page.
    pub args_preview: String,
    /// Who requested the approval (the operator actor).
    pub requested_by: String,
    /// The tool's declared risk level at request time.
    pub risk: RiskLevel,
    pub created_at: String,
    /// Set once a single execution attempt has been made; a consumed binding can
    /// never run again (one-shot).
    pub consumed: bool,
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
    /// Operator-curated MCP server registrations (loopback HTTP only), keyed by
    /// server id. MCP v1: a discovery surface — the kernel lists these and runs a
    /// live `tools/list` against an enabled one. No secrets are stored.
    mcp_servers: HashMap<String, relux_core::McpServerConfig>,
    namespaces: HashMap<NamespaceId, Namespace>,
    agents: HashMap<AgentId, Agent>,
    tasks: HashMap<TaskId, Task>,
    runs: HashMap<RunId, Run>,
    pub approvals: HashMap<ApprovalId, Approval>,
    /// Per-tool-call approval bindings, keyed by the approval id they belong to.
    /// Created by [`request_tool_invocation_approval`](KernelState::request_tool_invocation_approval)
    /// and consumed once by
    /// [`execute_approved_tool_invocation`](KernelState::execute_approved_tool_invocation).
    pending_tool_invocations: HashMap<ApprovalId, PendingToolInvocation>,
    /// Persistent allow-always grants, keyed by grant id. Created by
    /// [`grant_persistent_tool_invocation`](KernelState::grant_persistent_tool_invocation)
    /// and consulted at the per-call approval gate in
    /// [`call_tool`](KernelState::call_tool) / [`invoke_tool`](KernelState::invoke_tool)
    /// so a future matching invocation bypasses the prompt; removed by
    /// [`revoke_persistent_grant`](KernelState::revoke_persistent_grant).
    persistent_grants: HashMap<String, PersistentGrant>,
    /// Durable Prime orchestrations, keyed by id.
    orchestrations: HashMap<OrchestrationId, Orchestration>,
    /// Per-run transcripts, in emission order.
    run_events: Vec<RunEvent>,
    /// Bounded, redacted per-run log tails, keyed by run id. Captured at run
    /// finalize from the adapter's already-redacted, byte-capped stdout/stderr
    /// plus kernel-authored `system` lines (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md`
    /// §8/§10 — the live run-log/tail surface). One bounded [`relux_core::RunLog`]
    /// per run; read-only projection via [`KernelState::run_log`].
    run_logs: HashMap<RunId, relux_core::RunLog>,
    /// The append-only audit log, in emission order.
    audit_log: Vec<AuditEvent>,
    pub prime_autonomy_config: PrimeAutonomyConfig,
    /// Multi-turn clarification memory: the small, bounded pending-clarification
    /// record per conversation (keyed by namespace + actor), so a follow-up answer
    /// can resolve the clarifying question Prime asked last turn instead of being
    /// read as a fresh, context-free message (`docs/prime-processing-audit.md`
    /// "Multi-turn clarify memory"; see [`crate::prime_clarify_memory`]). Bounded:
    /// the latest record per conversation, with a hard cap on total entries.
    pending_clarifications: HashMap<String, relux_core::PendingClarification>,
    /// Bounded conversation history: the last few recorded turns per conversation (keyed by
    /// namespace + actor), so the NEXT turn's brain can interpret a follow-up in context
    /// instead of reasoning only from the bare current message (`docs/prime-processing-audit.md`
    /// "Bounded conversation memory"; see [`crate::prime_history`]). Advisory grounding only —
    /// it is rendered into the brain's prompt as background, never consulted by the deterministic
    /// classifier, the fail-closed intent gate, or any existence/approval check. Bounded:
    /// [`crate::prime_history::MAX_HISTORY_TURNS`] per conversation,
    /// [`crate::prime_history::MAX_HISTORY_CONVERSATIONS`] overall, every field secret-redacted.
    conversation_histories: HashMap<String, Vec<relux_core::ConversationTurn>>,
    /// Rolling, bounded, deterministic per-conversation summary of the turns that have aged OUT
    /// of [`conversation_histories`](Self::conversation_histories) (keyed the same
    /// `namespace::actor`), so a long-running Prime thread keeps a compact memory of older turns
    /// instead of forgetting them when the ring evicts (`docs/prime-processing-audit.md`
    /// "Bounded conversation-memory compaction"; see [`crate::prime_history::fold_evicted_turn`]).
    /// Advisory grounding ONLY — like the ring it is rendered into the brain's prompt as
    /// background and is never consulted by the deterministic classifier, the fail-closed intent
    /// gate, or any existence/approval check. Every field is bounded + secret-redacted.
    conversation_summaries: HashMap<String, relux_core::ConversationSummary>,
    clock: Clock,
    next_task: u64,
    next_run: u64,
    next_approval: u64,
    next_audit: u64,
    next_event: u64,
    next_orchestration: u64,
    next_grant: u64,
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
            mcp_servers: sorted(&self.mcp_servers, |c| c.id.as_str()),
            namespaces: sorted(&self.namespaces, |n| n.id.as_str()),
            agents: sorted(&self.agents, |a| a.id.as_str()),
            tasks: sorted(&self.tasks, |t| t.id.as_str()),
            runs: sorted(&self.runs, |r| r.id.as_str()),
            approvals: sorted(&self.approvals, |a| a.id.as_str()),
            pending_tool_invocations: sorted(&self.pending_tool_invocations, |p| {
                p.approval_id.as_str()
            }),
            persistent_grants: sorted(&self.persistent_grants, |g| g.id.as_str()),
            run_events: self.run_events.clone(),
            run_logs: sorted(&self.run_logs, |l| l.run_id.as_str()),
            audit_events: self.audit_log.clone(),
            prime_autonomy_config: self.prime_autonomy_config.clone(),
            orchestrations: sorted(&self.orchestrations, |o| o.id.as_str()),
            pending_clarifications: {
                let mut out: Vec<PendingClarificationEntry> = self
                    .pending_clarifications
                    .iter()
                    .map(|(key, pending)| PendingClarificationEntry {
                        key: key.clone(),
                        pending: pending.clone(),
                    })
                    .collect();
                out.sort_by(|a, b| a.key.cmp(&b.key));
                out
            },
            conversation_histories: {
                let mut out: Vec<ConversationHistoryEntry> = self
                    .conversation_histories
                    .iter()
                    .map(|(key, turns)| ConversationHistoryEntry {
                        key: key.clone(),
                        turns: turns.clone(),
                    })
                    .collect();
                out.sort_by(|a, b| a.key.cmp(&b.key));
                out
            },
            conversation_summaries: {
                let mut out: Vec<ConversationSummaryEntry> = self
                    .conversation_summaries
                    .iter()
                    .map(|(key, summary)| ConversationSummaryEntry {
                        key: key.clone(),
                        summary: summary.clone(),
                    })
                    .collect();
                out.sort_by(|a, b| a.key.cmp(&b.key));
                out
            },
            counters: KernelCounters {
                clock_secs: self.clock.secs(),
                next_task: self.next_task,
                next_run: self.next_run,
                next_approval: self.next_approval,
                next_audit: self.next_audit,
                next_event: self.next_event,
                next_orchestration: self.next_orchestration,
                next_grant: self.next_grant,
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
        for cfg in snapshot.mcp_servers {
            state.mcp_servers.insert(cfg.id.clone(), cfg);
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
        for pending in snapshot.pending_tool_invocations {
            state
                .pending_tool_invocations
                .insert(pending.approval_id.clone(), pending);
        }
        for grant in snapshot.persistent_grants {
            state.persistent_grants.insert(grant.id.clone(), grant);
        }
        for orchestration in snapshot.orchestrations {
            state
                .orchestrations
                .insert(orchestration.id.clone(), orchestration);
        }
        for entry in snapshot.pending_clarifications {
            state.pending_clarifications.insert(entry.key, entry.pending);
        }
        for entry in snapshot.conversation_histories {
            state.conversation_histories.insert(entry.key, entry.turns);
        }
        for entry in snapshot.conversation_summaries {
            state.conversation_summaries.insert(entry.key, entry.summary);
        }
        state.run_events = snapshot.run_events;
        for log in snapshot.run_logs {
            state.run_logs.insert(log.run_id.clone(), log);
        }
        state.audit_log = snapshot.audit_events;
        state.clock = Clock::from_secs(snapshot.counters.clock_secs);
        state.next_task = snapshot.counters.next_task;
        state.next_run = snapshot.counters.next_run;
        state.next_approval = snapshot.counters.next_approval;
        state.next_audit = snapshot.counters.next_audit;
        state.next_event = snapshot.counters.next_event;
        state.next_orchestration = snapshot.counters.next_orchestration;
        state.next_grant = snapshot.counters.next_grant;
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

    // --- Operator-configured plugin tools ----------------------------------

    /// Add or replace ONE operator-configured tool on a user-installed ToolSet
    /// plugin manifest (`docs/RELUX_MASTER_PLAN.md` §7.4 Plugin Kernel Layer, §8.2
    /// ToolSet Plugins). This is the kernel half of the in-UI "add a tool" form
    /// that makes a metadata-only wrapper useful without re-installing.
    ///
    /// Fail-closed by construction:
    /// - the plugin must be INSTALLED (`PluginNotInstalled`);
    /// - a BUNDLED/protected plugin is refused (its tools are built-in);
    /// - only a `ToolSet` may be configured (an Adapter is configured through the
    ///   adapter-runtime path, not here);
    /// - the [`ToolDefinition`] is built by [`crate::plugin_tool_config`], so the
    ///   permission is DERIVED (`tool:<plugin-id>:<verb>`), never operator-supplied,
    ///   and the approval requirement is risk-driven;
    /// - the whole manifest is re-validated before the change stands (so a
    ///   configured tool can never leave the manifest malformed).
    ///
    /// The change is applied transactionally on a clone: a validation failure
    /// leaves the live manifest untouched. The mutated manifest persists through
    /// the store like any other manifest (the install store is authoritative for a
    /// user plugin; the bundled refresh never touches it). Returns the stored
    /// [`ToolDefinition`].
    pub fn configure_plugin_tool(
        &mut self,
        plugin_id: &PluginId,
        input: crate::plugin_tool_config::PluginToolInput,
    ) -> Result<ToolDefinition, KernelError> {
        // 1. Must be installed, and must not be a protected bundled fixture.
        let installed = self
            .installed_plugins
            .get(plugin_id)
            .ok_or_else(|| KernelError::PluginNotInstalled(plugin_id.to_string()))?;
        if installed.source_kind == PluginSourceKind::Bundled {
            return Err(KernelError::BundledPluginProtected(plugin_id.to_string()));
        }

        // 2. Build the tool definition (this derives the plugin-scoped permission).
        let tool = input.into_tool_definition(plugin_id.as_str()).map_err(|message| {
            KernelError::InvalidToolDefinition {
                plugin: plugin_id.to_string(),
                message,
            }
        })?;

        // 3. Apply on a clone so a validation failure never leaves the live
        //    manifest mutated. Only a ToolSet may carry tools.
        let mut updated = self
            .plugins
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| KernelError::PluginNotInstalled(plugin_id.to_string()))?;
        if updated.kind != PluginKind::ToolSet {
            return Err(KernelError::PluginNotToolConfigurable {
                plugin: plugin_id.to_string(),
                message: format!(
                    "only ToolSet plugins can have tools configured here; this plugin is a {:?}",
                    updated.kind
                ),
            });
        }

        // Upsert the tool by name, and ensure its derived permission is declared.
        if let Some(existing) = updated
            .capabilities
            .tools
            .iter_mut()
            .find(|t| t.name == tool.name)
        {
            *existing = tool.clone();
        } else {
            updated.capabilities.tools.push(tool.clone());
        }
        if !updated
            .capabilities
            .permissions
            .iter()
            .any(|p| p.matches_exact(&tool.permission))
        {
            updated.capabilities.permissions.push(tool.permission.clone());
        }

        // 4. Re-validate the WHOLE manifest before committing (defense in depth).
        relux_core::plugin::validate_manifest(&updated).map_err(|source| {
            KernelError::ManifestInvalid {
                path: plugin_id.to_string(),
                source,
            }
        })?;

        self.plugins.insert(plugin_id.clone(), updated);
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:tool_configure",
            Some("plugin"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::json!({
                "tool": tool.name,
                "permission": tool.permission.as_str(),
                "risk": format!("{:?}", tool.risk),
                "approval": format!("{:?}", tool.approval),
            }),
        );
        Ok(tool)
    }

    /// Remove ONE operator-configured tool from a user-installed ToolSet plugin
    /// manifest by name. Symmetric with [`configure_plugin_tool`]: bundled plugins
    /// are refused, an unknown tool is a clear error, and the manifest is mutated
    /// transactionally on a clone and re-validated before it stands. The tool's
    /// derived permission is dropped too when no other tool still references it.
    pub fn remove_plugin_tool(
        &mut self,
        plugin_id: &PluginId,
        tool_name: &str,
    ) -> Result<(), KernelError> {
        let installed = self
            .installed_plugins
            .get(plugin_id)
            .ok_or_else(|| KernelError::PluginNotInstalled(plugin_id.to_string()))?;
        if installed.source_kind == PluginSourceKind::Bundled {
            return Err(KernelError::BundledPluginProtected(plugin_id.to_string()));
        }

        let mut updated = self
            .plugins
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| KernelError::PluginNotInstalled(plugin_id.to_string()))?;

        let before = updated.capabilities.tools.len();
        let removed_permission = updated
            .capabilities
            .tools
            .iter()
            .find(|t| t.name == tool_name)
            .map(|t| t.permission.clone());
        updated.capabilities.tools.retain(|t| t.name != tool_name);
        if updated.capabilities.tools.len() == before {
            return Err(KernelError::PluginToolNotFound {
                plugin: plugin_id.to_string(),
                tool: tool_name.to_string(),
            });
        }

        // Drop the tool's permission only when no remaining tool still needs it.
        if let Some(perm) = removed_permission {
            let still_used = updated
                .capabilities
                .tools
                .iter()
                .any(|t| t.permission.matches_exact(&perm));
            if !still_used {
                updated
                    .capabilities
                    .permissions
                    .retain(|p| !p.matches_exact(&perm));
            }
        }

        relux_core::plugin::validate_manifest(&updated).map_err(|source| {
            KernelError::ManifestInvalid {
                path: plugin_id.to_string(),
                source,
            }
        })?;

        self.plugins.insert(plugin_id.clone(), updated);
        self.record_audit(
            "kernel",
            "kernel",
            "plugin:tool_remove",
            Some("plugin"),
            Some(plugin_id.as_str()),
            None,
            AuditResult::Success,
            serde_json::json!({ "tool": tool_name }),
        );
        Ok(())
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

    // --- MCP servers (loopback HTTP discovery — MCP v1) --------------------
    // `docs/RELUX_MASTER_PLAN.md` §8.2/§18, `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9,
    // `docs/mcp.md`. The kernel registers operator-curated, loopback-only MCP
    // servers and runs a live `tools/list` against an enabled one. No secrets are
    // stored. MCP tool INVOCATION routes through the SAME tool-invoke gates as a
    // plugin tool (permission / risk-approval / per-call / grant / audit), under
    // `plugin_id = "mcp:<server>"`; a discovered tool's executable state is driven by
    // its operator classification (gated by default until classified).

    /// Register (or replace) an MCP server. The id + loopback endpoint are validated
    /// with the same loopback-only rule as the plugin runtime; the description is
    /// sanitized and the timeout clamped. Upsert by id so re-registering updates the
    /// endpoint/description/enabled flag in place. Stores no secrets.
    pub fn register_mcp_server(
        &mut self,
        id: &str,
        endpoint: &str,
        description: &str,
        enabled: bool,
        timeout_ms: Option<u64>,
    ) -> Result<relux_core::McpServerConfig, KernelError> {
        let id = id.trim().to_string();
        // Upsert preserves any operator-set per-tool classifications so re-pointing a
        // server's endpoint/description does not silently reset its tools' risk.
        let tool_overrides = self
            .mcp_servers
            .get(&id)
            .map(|existing| existing.tool_overrides.clone())
            .unwrap_or_default();
        let config = relux_core::McpServerConfig {
            id: id.clone(),
            transport: relux_core::McpTransport::HttpLoopback,
            endpoint: endpoint.trim().to_string(),
            description: relux_core::sanitize_mcp_text(
                description,
                relux_core::MAX_MCP_DESCRIPTION_CHARS,
            ),
            enabled,
            timeout_ms: relux_core::clamp_mcp_timeout(timeout_ms),
            tool_overrides,
        };
        relux_core::validate_mcp_server_config(&config).map_err(|e| {
            KernelError::InvalidMcpConfig {
                id: id.clone(),
                message: e.to_string(),
            }
        })?;
        self.record_audit(
            "kernel",
            "kernel",
            "mcp:server_register",
            Some("mcp_server"),
            Some(&config.id),
            None,
            AuditResult::Success,
            serde_json::json!({
                "transport": config.transport.as_str(),
                "enabled": config.enabled,
                "timeout_ms": config.timeout_ms,
            }),
        );
        self.mcp_servers.insert(config.id.clone(), config.clone());
        Ok(config)
    }

    /// Toggle an MCP server's enabled flag, keeping its endpoint. Errors if unknown.
    pub fn set_mcp_server_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<relux_core::McpServerConfig, KernelError> {
        let server = self
            .mcp_servers
            .get_mut(id)
            .ok_or_else(|| KernelError::UnknownMcpServer(id.to_string()))?;
        server.enabled = enabled;
        let server = server.clone();
        self.record_audit(
            "kernel",
            "kernel",
            if enabled { "mcp:server_enable" } else { "mcp:server_disable" },
            Some("mcp_server"),
            Some(id),
            None,
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(server)
    }

    /// Remove an MCP server registration entirely. Errors if unknown.
    pub fn remove_mcp_server(&mut self, id: &str) -> Result<(), KernelError> {
        self.mcp_servers
            .remove(id)
            .ok_or_else(|| KernelError::UnknownMcpServer(id.to_string()))?;
        self.record_audit(
            "kernel",
            "kernel",
            "mcp:server_remove",
            Some("mcp_server"),
            Some(id),
            None,
            AuditResult::Success,
            serde_json::Value::Null,
        );
        Ok(())
    }

    /// One MCP server config by id.
    pub fn mcp_server(&self, id: &str) -> Option<&relux_core::McpServerConfig> {
        self.mcp_servers.get(id)
    }

    /// All MCP servers, sorted by id for deterministic listing.
    pub fn mcp_servers(&self) -> Vec<&relux_core::McpServerConfig> {
        let mut out: Vec<&relux_core::McpServerConfig> = self.mcp_servers.values().collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Run a live `tools/list` discovery against an enabled MCP server and map the
    /// result into [`ToolDescriptor`]s for the unified Tools surface.
    ///
    /// Honest by construction: an unknown server is [`KernelError::UnknownMcpServer`];
    /// a disabled one is [`KernelError::McpServerDisabled`]; a transport/protocol
    /// failure is [`KernelError::McpDiscoveryFailed`] (never a fabricated empty
    /// list). Each discovered tool's executable state is driven by its
    /// [`relux_core::McpToolClassification`]: `needs_approval` until the operator
    /// classifies it, `ready` once classified low-risk + auto-approve. Invocation
    /// itself routes through the standard tool-invoke gates (`docs/mcp.md`).
    /// Read-only: it performs a bounded loopback network call but mutates no state.
    pub fn discover_mcp_tools(&self, id: &str) -> Result<Vec<ToolDescriptor>, KernelError> {
        let server = self
            .mcp_servers
            .get(id)
            .ok_or_else(|| KernelError::UnknownMcpServer(id.to_string()))?;
        if !server.enabled {
            return Err(KernelError::McpServerDisabled(id.to_string()));
        }
        let tools = crate::mcp::discover_tools(&server.endpoint, server.timeout_ms).map_err(|e| {
            KernelError::McpDiscoveryFailed {
                id: id.to_string(),
                message: e.to_string(),
            }
        })?;
        let plugin_id = relux_core::mcp_synthetic_plugin_id(id);
        let mut out: Vec<ToolDescriptor> = tools
            .into_iter()
            .map(|tool| {
                // Scan the (untrusted) tool description for prompt-injection
                // patterns; log a warning but still list the tool (mirrors Hermes
                // `_scan_mcp_description` — advisory, never a block).
                let findings = relux_core::scan_mcp_tool_description(&tool.description);
                if !findings.is_empty() {
                    // Advisory, never a block (false positives would break
                    // legitimate servers). Surfaced on stderr so an operator can see
                    // a suspicious MCP tool description without a logging dependency.
                    eprintln!(
                        "[relux] WARN mcp server '{id}' tool '{}': suspicious description content — {}",
                        tool.name,
                        findings.join("; ")
                    );
                }
                // The tool's risk + approval come from the operator's classification
                // (or the fail-closed default: Medium + Required). The SAME
                // `approval_blocks_direct_invocation` predicate that gates real
                // plugin tools decides whether this MCP tool is directly runnable or
                // gated behind approval — so an unclassified tool reads as
                // `needs_approval`, never auto-runnable.
                let classification = server.tool_classification(&tool.name);
                let executable = if approval_blocks_direct_invocation(
                    &classification.approval,
                    &classification.risk,
                ) {
                    ToolExecutability::NeedsApproval
                } else {
                    // Low-risk + auto-approve: directly invocable. The MCP server
                    // (enabled, loopback) IS the runtime; the invoke path still
                    // permission-checks the calling agent.
                    ToolExecutability::Ready
                };
                ToolDescriptor {
                    permission: relux_core::mcp_tool_permission(id, &tool.name),
                    plugin_id: plugin_id.clone(),
                    tool_name: tool.name,
                    description: tool.description,
                    risk: classification.risk,
                    source_kind: "Mcp".to_string(),
                    installed: true,
                    enabled: server.enabled,
                    protected: false,
                    executable,
                }
            })
            .collect();
        out.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        Ok(out)
    }

    /// Set (or replace) the operator's risk + approval classification for ONE MCP
    /// tool, so a discovered tool can become directly runnable (Low + auto-approve)
    /// or stay gated. The server must exist and the tool name must be a safe
    /// identifier ([`relux_core::is_valid_mcp_tool_name`]). Persisted on the server
    /// config and audited. This is the MCP analogue of declaring a `ToolDefinition`
    /// risk/approval, and the same `approval_blocks_direct_invocation` predicate
    /// then decides whether the tool gates.
    pub fn set_mcp_tool_classification(
        &mut self,
        server_id: &str,
        tool_name: &str,
        risk: relux_core::RiskLevel,
        approval: relux_core::ApprovalRequirement,
    ) -> Result<relux_core::McpServerConfig, KernelError> {
        if !relux_core::is_valid_mcp_tool_name(tool_name) {
            return Err(KernelError::InvalidMcpToolName {
                server: server_id.to_string(),
                tool: tool_name.to_string(),
            });
        }
        let server = self
            .mcp_servers
            .get_mut(server_id)
            .ok_or_else(|| KernelError::UnknownMcpServer(server_id.to_string()))?;
        server.tool_overrides.insert(
            tool_name.to_string(),
            relux_core::McpToolClassification {
                risk: risk.clone(),
                approval: approval.clone(),
            },
        );
        let server = server.clone();
        self.record_audit(
            "kernel",
            "kernel",
            "mcp:tool_classify",
            Some("mcp_tool"),
            Some(tool_name),
            None,
            AuditResult::Success,
            serde_json::json!({
                "server": server_id,
                "risk": format!("{risk:?}"),
                "approval": format!("{approval:?}"),
            }),
        );
        Ok(server)
    }

    /// Remove an MCP tool's operator classification, reverting it to the fail-closed
    /// default (Medium + Required → gated). The server must exist; clearing an
    /// unclassified tool is a no-op success. Audited.
    pub fn clear_mcp_tool_classification(
        &mut self,
        server_id: &str,
        tool_name: &str,
    ) -> Result<relux_core::McpServerConfig, KernelError> {
        let server = self
            .mcp_servers
            .get_mut(server_id)
            .ok_or_else(|| KernelError::UnknownMcpServer(server_id.to_string()))?;
        server.tool_overrides.remove(tool_name);
        let server = server.clone();
        self.record_audit(
            "kernel",
            "kernel",
            "mcp:tool_unclassify",
            Some("mcp_tool"),
            Some(tool_name),
            None,
            AuditResult::Success,
            serde_json::json!({ "server": server_id }),
        );
        Ok(server)
    }

    /// If `plugin_id` is an MCP synthetic plugin id (`mcp:<server>`), the registered
    /// server config, else `None`. This is the single chokepoint the tool-invocation
    /// gates use to recognise an MCP tool: every gate first asks "is this an MCP
    /// plugin?" and, if so, resolves permission/risk/approval/runtime from the MCP
    /// server config instead of an installed plugin manifest.
    fn mcp_server_for_plugin(&self, plugin_id: &PluginId) -> Option<&relux_core::McpServerConfig> {
        let server_id = plugin_id.as_str().strip_prefix("mcp:")?;
        self.mcp_servers.get(server_id)
    }

    /// The risk level a tool's per-call approval / persistent grant snapshots.
    /// MCP-aware: a `mcp:<server>` tool uses the operator's classification (or the
    /// fail-closed default Medium); a real plugin tool uses its manifest-declared
    /// risk, defaulting to High when the plugin/tool cannot be resolved (fail
    /// closed). Shared by [`request_tool_invocation_approval`](Self::request_tool_invocation_approval)
    /// and [`grant_persistent_tool_invocation`](Self::grant_persistent_tool_invocation).
    fn tool_risk_for(&self, plugin_id: &PluginId, tool_name: &str) -> RiskLevel {
        if let Some(server) = self.mcp_server_for_plugin(plugin_id) {
            return server.tool_classification(tool_name).risk;
        }
        self.plugins
            .get(plugin_id)
            .and_then(|m| m.capabilities.tools.iter().find(|t| t.name == tool_name))
            .map(|t| t.risk.clone())
            .unwrap_or(RiskLevel::High)
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
    /// This is the original (skill-less) entry point — it delegates to
    /// [`Self::create_agent_with_skills`] with an empty skill list so every existing
    /// caller keeps working unchanged (backwards compatible).
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
        self.create_agent_with_skills(
            id,
            name,
            description,
            adapter_plugin,
            namespace,
            persona,
            permissions,
            Vec::new(),
            None,
        )
    }

    /// The current org lattice as a child→Lead (`reports_to`) map, for the pure
    /// [`relux_core::hierarchy`] walks (cycle/subtree). Built fresh from the live roster.
    fn reports_to_map(&self) -> relux_core::hierarchy::ReportsToMap {
        self.agents
            .values()
            .filter_map(|a| a.reports_to.clone().map(|m| (a.id.clone(), m)))
            .collect()
    }

    /// Create a configured agent actor carrying bounded specialty `skills`/tags (the
    /// manual Crew-config path). Skills must already be validated/sanitized by
    /// [`crate::agent_config::validate_skills`]; this method just stores them.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn create_agent_with_skills(
        &mut self,
        id: &str,
        name: &str,
        description: &str,
        adapter_plugin: &PluginId,
        namespace: &NamespaceId,
        persona: Option<String>,
        permissions: Vec<Permission>,
        skills: Vec<String>,
        reports_to: Option<AgentId>,
    ) -> Result<AgentId, KernelError> {
        if !self.plugins.contains_key(adapter_plugin) {
            return Err(KernelError::UnknownPlugin(adapter_plugin.to_string()));
        }
        let agent_id = AgentId::new(id);
        if self.agents.contains_key(&agent_id) {
            return Err(KernelError::AgentExists(agent_id.to_string()));
        }
        // The Lead must be an existing operative and cannot be self. A fresh operative is
        // a leaf, so it can never close a cycle — existence + self is the whole check.
        if let Some(ref lead) = reports_to {
            if lead == &agent_id {
                return Err(KernelError::InvalidAgentConfig(
                    "an operative cannot report to itself".to_string(),
                ));
            }
            if !self.agents.contains_key(lead) {
                return Err(KernelError::InvalidAgentConfig(format!(
                    "unknown manager '{lead}'; choose an existing crew member as the Lead"
                )));
            }
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
            skills,
            reports_to,
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

    /// Apply an operator edit to an existing agent's configurable fields.
    ///
    /// Each argument is "leave unchanged" when `None`; for `persona`, `Some(None)`
    /// CLEARS it and `Some(Some(_))` sets it. The caller (the HTTP layer) is
    /// responsible for sanitizing/validating the values via
    /// [`crate::agent_config::validate_agent_update`]; this method enforces the two
    /// invariants the kernel owns: the agent must exist, and a new adapter must be an
    /// installed plugin (the manual-config counterpart to `create_agent`'s check).
    /// The brain-seeded create path is untouched — this is edit-only. This original
    /// signature delegates to [`Self::update_agent_with_skills`] with `skills = None`
    /// (leave skills unchanged), so existing callers keep working unchanged.
    #[allow(clippy::too_many_arguments)]
    pub fn update_agent(
        &mut self,
        id: &AgentId,
        name: Option<String>,
        description: Option<String>,
        persona: Option<Option<String>>,
        adapter_plugin: Option<PluginId>,
        status: Option<AgentStatus>,
    ) -> Result<(), KernelError> {
        self.update_agent_with_skills(
            id,
            name,
            description,
            persona,
            adapter_plugin,
            status,
            None,
            None,
        )
    }

    /// Apply an operator edit including the optional specialty `skills`/tags and the
    /// optional Lead (`reports_to`): `None` leaves a field unchanged, `Some(list)`
    /// REPLACES the whole skill list (an empty list clears it), and for the Lead
    /// `Some(None)` clears it (top-level) while `Some(Some(lead))` sets it. Skills and
    /// the Lead id must already be sanitized/resolved by
    /// [`crate::agent_config::validate_agent_update`]; this method enforces the graph
    /// invariants the kernel owns: a set Lead must be an existing operative, cannot be
    /// self, and must not create a reporting cycle (a re-point under one's own Branch).
    #[allow(clippy::too_many_arguments)]
    pub fn update_agent_with_skills(
        &mut self,
        id: &AgentId,
        name: Option<String>,
        description: Option<String>,
        persona: Option<Option<String>>,
        adapter_plugin: Option<PluginId>,
        status: Option<AgentStatus>,
        skills: Option<Vec<String>>,
        reports_to: Option<Option<AgentId>>,
    ) -> Result<(), KernelError> {
        if !self.agents.contains_key(id) {
            return Err(KernelError::UnknownAgent(id.to_string()));
        }
        if let Some(ref plugin) = adapter_plugin {
            if !self.plugins.contains_key(plugin) {
                return Err(KernelError::UnknownPlugin(plugin.to_string()));
            }
        }
        // Validate a SET Lead before mutating anything (a clear/unchanged needs no check).
        if let Some(Some(ref lead)) = reports_to {
            if lead == id {
                return Err(KernelError::InvalidAgentConfig(
                    "an operative cannot report to itself".to_string(),
                ));
            }
            if !self.agents.contains_key(lead) {
                return Err(KernelError::InvalidAgentConfig(format!(
                    "unknown manager '{lead}'; choose an existing crew member as the Lead"
                )));
            }
            // Pointing `id` → `lead` must not close a loop (e.g. making a manager report
            // to one of its own reports). Checked against the live lattice via the pure
            // helper, bounded-depth and total even on a malformed map.
            if relux_core::hierarchy::would_create_cycle(id, lead, &self.reports_to_map()) {
                return Err(KernelError::InvalidAgentConfig(format!(
                    "setting Lead to '{lead}' would create a reporting cycle"
                )));
            }
        }

        let (namespace, adapter_label) = {
            // Safe: existence checked above.
            let agent = self.agents.get_mut(id).expect("agent exists");
            if let Some(n) = name {
                agent.name = n;
            }
            if let Some(d) = description {
                agent.description = d;
            }
            if let Some(p) = persona {
                agent.persona = p;
            }
            if let Some(a) = adapter_plugin {
                agent.adapter_plugin = a;
            }
            if let Some(s) = status {
                agent.status = s;
            }
            if let Some(sk) = skills {
                agent.skills = sk;
            }
            if let Some(lead) = reports_to {
                agent.reports_to = lead;
            }
            (
                agent.namespace_id.clone(),
                agent.adapter_plugin.as_str().to_string(),
            )
        };
        self.record_audit(
            "kernel",
            "kernel",
            "agent:update",
            Some("agent"),
            Some(id.as_str()),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({ "adapter": adapter_label }),
        );
        Ok(())
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

    /// Revoke an explicit permission from an existing agent (the inverse of
    /// [`grant_permission_to_agent`]). The operator console is the human approval —
    /// the action is audited like the grant. Revoking a permission the agent does not
    /// hold is an honest [`KernelError::PermissionNotGranted`] (fail closed: we never
    /// silently report success for a no-op). A revoke only removes an EXPLICIT grant;
    /// it cannot reach implicit/built-in capabilities (there are none — least
    /// privilege, section 17.5), so the agent's effective powers shrink to exactly the
    /// remaining explicit list.
    pub fn revoke_permission_from_agent(
        &mut self,
        agent_id: &AgentId,
        permission: &Permission,
    ) -> Result<(), KernelError> {
        let agent = self
            .agents
            .get_mut(agent_id)
            .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?;

        let before = agent.permissions.len();
        agent.permissions.retain(|p| !p.matches_exact(permission));
        if agent.permissions.len() == before {
            return Err(KernelError::PermissionNotGranted(
                agent_id.to_string(),
                permission.to_string(),
            ));
        }
        let namespace_id = agent.namespace_id.clone();

        self.record_audit(
            "kernel",
            "kernel",
            "agent:revoke_permission",
            Some("agent"),
            Some(agent_id.as_str()),
            Some(&namespace_id),
            AuditResult::Success,
            serde_json::json!({ "permission": permission.as_str() }),
        );

        Ok(())
    }

    /// True iff `manager` may exercise the manager-subtree control `action` on `target`:
    /// it holds a `agent:<manager>:subtree:<action>` grant over its OWN Branch AND `target`
    /// is a proper descendant of `manager` in the live `reports_to` lattice (the bounded
    /// [`relux_core::hierarchy::is_in_subtree`] walk).
    ///
    /// This is the single chokepoint for manager-subtree authority — the kernel half of
    /// Paperclip `scopeAllows` + `agentIsInSubtree`. It layers a fail-closed enablement
    /// rule on top of the pure grammar matcher: **only an `Active` manager wields authority**
    /// (a `Draft`/`Paused`/`Disabled`/`Error` manager is denied — the org lattice and a
    /// disabled actor's powers are orthogonal, and the safe default for *exercising* a power
    /// is to require the actor be live). An unknown manager or target is denied.
    fn manager_subtree_authorizes(
        &self,
        manager: &AgentId,
        action: &str,
        target: &AgentId,
    ) -> bool {
        let Some(manager_agent) = self.agents.get(manager) else {
            return false;
        };
        // Fail-closed: a non-Active manager exercises no subtree authority.
        if manager_agent.status != AgentStatus::Active {
            return false;
        }
        // The target must be a real operative.
        if !self.agents.contains_key(target) {
            return false;
        }
        let reports_to = self.reports_to_map();
        manager_agent.permissions.iter().any(|grant| {
            relux_core::permission::manager_subtree_authorizes(
                grant, manager, action, target, &reports_to,
            )
        })
    }

    /// A manager agent grants a permission to one of its (transitive) subordinates,
    /// authorized by a manager-subtree scope (`agent:<manager>:subtree:grant_permission`)
    /// it holds over its own Branch and `target` actually sitting in that Branch. This is
    /// the one real enforcement path that consults `reports_to` for *authority* (the
    /// previously display-only org lattice now gates a real mutation here).
    ///
    /// It does NOT widen the operator-console path: [`grant_permission_to_agent`] stays a
    /// kernel/operator action with no actor gate. This adds a strictly *narrower*
    /// agent-authority path — a manager can only reach operatives inside its own subtree,
    /// only for the `grant_permission` action it was scoped, and only while Active. The
    /// underlying grant still goes through `grant_permission_to_agent` (exact-match dedup,
    /// audited). An unauthorized manager is denied + audited and grants nothing.
    pub fn manager_grant_permission_to_subordinate(
        &mut self,
        manager_id: &AgentId,
        target_id: &AgentId,
        permission: Permission,
    ) -> Result<(), KernelError> {
        if !self.manager_subtree_authorizes(manager_id, "grant_permission", target_id) {
            let namespace = self.agents.get(target_id).map(|a| a.namespace_id.clone());
            self.record_audit(
                "agent",
                manager_id.as_str(),
                "agent:manager_grant_permission",
                Some("agent"),
                Some(target_id.as_str()),
                namespace.as_ref(),
                AuditResult::Denied,
                serde_json::json!({
                    "permission": permission.as_str(),
                    "reason": "manager-subtree authorization failed (not a live manager of the target's Branch)"
                }),
            );
            return Err(KernelError::PermissionDenied {
                agent: manager_id.to_string(),
                permission: format!("agent:{}:subtree:grant_permission", manager_id),
            });
        }
        // Authorized: perform the grant (audited as `agent:grant_permission` inside).
        self.grant_permission_to_agent(target_id, permission)
    }

    /// Operator-assisted manager grant: the same real manager-subtree authorization as
    /// [`Self::manager_grant_permission_to_subordinate`], but with the **operator** who
    /// stood in for the manager recorded in the audit trail.
    ///
    /// HONEST trust boundary: Relux has **no per-agent auth identity** yet — a manager
    /// agent cannot authenticate an HTTP request on its own behalf (OpenClaw correlates
    /// authority to a real per-session `sessionKey`/`spawnedBy`;
    /// `reference/openclaw-main/src/acp/session-lineage-meta.ts`). So an authenticated
    /// dashboard operator explicitly authorizes "grant *as* this manager". The operator
    /// can **not** bypass the manager-subtree rule: the grant of authority is still the
    /// real own-Branch + Active + scope check below; the operator only supplies the
    /// request and is named in the audit. This adds a `operator:authorize_manager_grant`
    /// audit row (Success/Denied) ON TOP OF the inner agent-actor audit — the operator
    /// view (who asked) and the agent view (which manager exercised authority) are both
    /// preserved.
    pub fn manager_grant_permission_to_subordinate_as_operator(
        &mut self,
        operator: &str,
        manager_id: &AgentId,
        target_id: &AgentId,
        permission: Permission,
    ) -> Result<(), KernelError> {
        let result = self.manager_grant_permission_to_subordinate(
            manager_id,
            target_id,
            permission.clone(),
        );
        let namespace = self.agents.get(target_id).map(|a| a.namespace_id.clone());
        self.record_audit(
            "operator",
            operator,
            "operator:authorize_manager_grant",
            Some("agent"),
            Some(target_id.as_str()),
            namespace.as_ref(),
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Denied
            },
            serde_json::json!({
                "manager_id": manager_id.as_str(),
                "permission": permission.as_str(),
                "trust_boundary": "operator console stood in for the manager (no per-agent auth identity yet); the manager-subtree authorization was NOT bypassed",
            }),
        );
        result
    }

    /// **Per-agent-authenticated** manager grant: the same real manager-subtree
    /// authorization as [`Self::manager_grant_permission_to_subordinate`], driven by a
    /// manager that authenticated its OWN request with a per-agent access token (no
    /// operator in the loop). This is the genuinely-per-agent path §19 of
    /// `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` called out as missing: the kernel trusts the
    /// authenticated agent identity as the acting manager, exactly as Paperclip attributes
    /// a request to `req.actor = { type: "agent", agentId: claims.sub }`
    /// (`references/paperclip/server/src/middleware/auth.ts`).
    ///
    /// `token_ref` is the **public, non-secret** token handle (`agt_<hex>`) that
    /// authenticated the request — recorded for provenance only; the raw token is NEVER
    /// passed here or logged. Authority is unchanged: the manager still only reaches
    /// operatives inside its own Branch, only for `grant_permission`, and only while
    /// Active (`manager_subtree_authorizes`). On top of the inner agent-actor audit
    /// (`agent:grant_permission` / `agent:manager_grant_permission`) this adds one
    /// `agent:token_authenticated_manager_grant` row (Success/Denied) marking that a
    /// per-agent token — not an operator — drove the grant.
    pub fn manager_grant_permission_to_subordinate_as_agent(
        &mut self,
        token_ref: &str,
        manager_id: &AgentId,
        target_id: &AgentId,
        permission: Permission,
    ) -> Result<(), KernelError> {
        let result = self.manager_grant_permission_to_subordinate(
            manager_id,
            target_id,
            permission.clone(),
        );
        let namespace = self.agents.get(target_id).map(|a| a.namespace_id.clone());
        self.record_audit(
            "agent",
            manager_id.as_str(),
            "agent:token_authenticated_manager_grant",
            Some("agent"),
            Some(target_id.as_str()),
            namespace.as_ref(),
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Denied
            },
            serde_json::json!({
                "permission": permission.as_str(),
                "auth_source": "agent_token",
                "token_ref": token_ref,
                "trust_boundary": "a per-agent access token authenticated the manager directly (no operator in the loop); the manager-subtree authorization was NOT bypassed",
            }),
        );
        result
    }

    /// A manager agent assigns an existing task to one of its (transitive) subordinates,
    /// authorized by a manager-subtree scope (`agent:<manager>:subtree:assign_task`) it
    /// holds over its OWN Branch and `target` actually sitting in that Branch. This is the
    /// **second** real enforcement path (after
    /// [`Self::manager_grant_permission_to_subordinate`]) that consults `reports_to` for
    /// *authority* — the same `manager_subtree_authorizes` gate, only the `action` differs.
    ///
    /// **Assignment semantics (deliberately the simple model the kernel already uses).**
    /// The task must EXIST and be **assignable** — i.e. not in a terminal state
    /// (`Completed`/`Failed`/`Cancelled`/`Expired`, per
    /// [`crate::prime_update_slots::is_terminal_status`]). On success the task's
    /// `assigned_agent` is set to `target` and it moves to `Queued` through the unchanged
    /// [`Self::assign_task`] (audited `task:assign`); a non-terminal task that was already
    /// assigned elsewhere is simply re-pointed, exactly as the operator/Prime path does.
    ///
    /// It does NOT widen the operator/Prime assignment path (those stay kernel/operator
    /// actions with no actor gate). It adds a strictly *narrower* agent-authority path: a
    /// manager can only reach operatives inside its own subtree, only for the `assign_task`
    /// action it was scoped, and only while Active. An unauthorized manager, a missing
    /// task, or a terminal task is denied + audited and assigns nothing. Authorization is
    /// checked FIRST, so an unauthorized manager never learns whether the task exists.
    pub fn manager_assign_task_to_subordinate(
        &mut self,
        manager_id: &AgentId,
        target_id: &AgentId,
        task_id: &TaskId,
    ) -> Result<(), KernelError> {
        // (1) Authority: own-Branch + Active + `assign_task` scope.
        if !self.manager_subtree_authorizes(manager_id, "assign_task", target_id) {
            let namespace = self.agents.get(target_id).map(|a| a.namespace_id.clone());
            self.record_audit(
                "agent",
                manager_id.as_str(),
                "agent:manager_assign_task",
                Some("task"),
                Some(task_id.as_str()),
                namespace.as_ref(),
                AuditResult::Denied,
                serde_json::json!({
                    "target": target_id.as_str(),
                    "reason": "manager-subtree authorization failed (not a live manager of the target's Branch)"
                }),
            );
            return Err(KernelError::PermissionDenied {
                agent: manager_id.to_string(),
                permission: format!("agent:{}:subtree:assign_task", manager_id),
            });
        }
        // (2) The task must exist and be assignable (not terminal). A terminal task is a
        //     resolvable conflict, audited so a denied (re)assignment is visible.
        match self.tasks.get(task_id) {
            None => return Err(KernelError::UnknownTask(task_id.to_string())),
            Some(task) if crate::prime_update_slots::is_terminal_status(&task.status) => {
                let status = format!("{:?}", task.status);
                let namespace = task.namespace_id.clone();
                self.record_audit(
                    "agent",
                    manager_id.as_str(),
                    "agent:manager_assign_task",
                    Some("task"),
                    Some(task_id.as_str()),
                    Some(&namespace),
                    AuditResult::Denied,
                    serde_json::json!({
                        "target": target_id.as_str(),
                        "reason": "task is in a terminal state and cannot be reassigned",
                        "status": status,
                    }),
                );
                return Err(KernelError::TaskNotAssignable {
                    task: task_id.to_string(),
                    status,
                });
            }
            Some(_) => {}
        }
        // (3) Authorized + assignable: perform the assignment (audited `task:assign` inside).
        self.assign_task(task_id, target_id)
    }

    /// **Per-agent-authenticated** task assignment: the same real manager-subtree
    /// authorization as [`Self::manager_assign_task_to_subordinate`], driven by a manager
    /// that authenticated its OWN request with a per-agent access token (no operator in the
    /// loop). The token actor analogue of
    /// [`Self::manager_grant_permission_to_subordinate_as_agent`] for the `assign_task`
    /// action.
    ///
    /// `token_ref` is the **public, non-secret** token handle (`agt_<hex>`) that
    /// authenticated the request — recorded for provenance only; the raw token is NEVER
    /// passed here or logged. Authority is unchanged (own-Branch + `Active` +
    /// `agent:<id>:subtree:assign_task` scope). On top of the inner agent-actor audit
    /// (`task:assign` on success / `agent:manager_assign_task` on a denial) this adds one
    /// `agent:token_authenticated_manager_assign_task` row (Success/Denied) marking that a
    /// per-agent token — not an operator — drove the assignment.
    pub fn manager_assign_task_to_subordinate_as_agent(
        &mut self,
        token_ref: &str,
        manager_id: &AgentId,
        target_id: &AgentId,
        task_id: &TaskId,
    ) -> Result<(), KernelError> {
        let result = self.manager_assign_task_to_subordinate(manager_id, target_id, task_id);
        let namespace = self.agents.get(target_id).map(|a| a.namespace_id.clone());
        self.record_audit(
            "agent",
            manager_id.as_str(),
            "agent:token_authenticated_manager_assign_task",
            Some("task"),
            Some(task_id.as_str()),
            namespace.as_ref(),
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Denied
            },
            serde_json::json!({
                "target": target_id.as_str(),
                "auth_source": "agent_token",
                "token_ref": token_ref,
                "trust_boundary": "a per-agent access token authenticated the manager directly (no operator in the loop); the manager-subtree authorization was NOT bypassed",
            }),
        );
        result
    }

    /// A manager agent revokes an explicit permission from one of its (transitive)
    /// subordinates, authorized by a manager-subtree scope
    /// (`agent:<manager>:subtree:revoke_permission`) it holds over its OWN Branch and
    /// `target` actually sitting in that Branch. The **third** real subtree-authority path
    /// (after `grant_permission` and `assign_task`) that consults `reports_to` for
    /// *authority* — the SAME `manager_subtree_authorizes` gate, only the `action` differs.
    ///
    /// The revoke removes EXACTLY the stored grant through the unchanged
    /// [`Self::revoke_permission_from_agent`] (`matches_exact` bookkeeping, audited
    /// `agent:revoke_permission`); it NEVER pattern-expands (a `tool:<plugin>:*` scope is
    /// only ever removed by revoking that exact scope row, not a concrete tool). If the
    /// target does NOT hold the exact permission it is the honest
    /// [`KernelError::PermissionNotGranted`] the operator revoke already returns (fail
    /// closed — never a silent no-op success). Authorization is checked FIRST, so an
    /// unauthorized manager never learns whether the target holds the permission.
    ///
    /// It does NOT widen the operator-console revoke ([`Self::revoke_permission_from_agent`]
    /// stays a kernel/operator action with no actor gate). It adds a strictly *narrower*
    /// agent-authority path: a manager can only reach operatives inside its own subtree,
    /// only for the `revoke_permission` action it was scoped, and only while Active. An
    /// unauthorized manager (no scope / not Active / target outside its Branch / unknown
    /// target) is denied + audited and revokes nothing.
    pub fn manager_revoke_permission_from_subordinate(
        &mut self,
        manager_id: &AgentId,
        target_id: &AgentId,
        permission: &Permission,
    ) -> Result<(), KernelError> {
        if !self.manager_subtree_authorizes(manager_id, "revoke_permission", target_id) {
            let namespace = self.agents.get(target_id).map(|a| a.namespace_id.clone());
            self.record_audit(
                "agent",
                manager_id.as_str(),
                "agent:manager_revoke_permission",
                Some("agent"),
                Some(target_id.as_str()),
                namespace.as_ref(),
                AuditResult::Denied,
                serde_json::json!({
                    "permission": permission.as_str(),
                    "reason": "manager-subtree authorization failed (not a live manager of the target's Branch)"
                }),
            );
            return Err(KernelError::PermissionDenied {
                agent: manager_id.to_string(),
                permission: format!("agent:{}:subtree:revoke_permission", manager_id),
            });
        }
        // Authorized: perform the revoke (audited as `agent:revoke_permission` inside; an
        // unheld permission is the honest PermissionNotGranted, never a silent no-op).
        self.revoke_permission_from_agent(target_id, permission)
    }

    /// **Per-agent-authenticated** permission revoke: the same real manager-subtree
    /// authorization as [`Self::manager_revoke_permission_from_subordinate`], driven by a
    /// manager that authenticated its OWN request with a per-agent access token (no
    /// operator in the loop). The token-actor analogue of
    /// [`Self::manager_grant_permission_to_subordinate_as_agent`] for the
    /// `revoke_permission` action.
    ///
    /// `token_ref` is the **public, non-secret** token handle (`agt_<hex>`) that
    /// authenticated the request — recorded for provenance only; the raw token is NEVER
    /// passed here or logged. Authority is unchanged (own-Branch + `Active` +
    /// `agent:<id>:subtree:revoke_permission` scope). On top of the inner agent-actor audit
    /// (`agent:revoke_permission` on success / `agent:manager_revoke_permission` on a
    /// denial) this adds one `agent:token_authenticated_manager_revoke_permission` row
    /// (Success/Denied) marking that a per-agent token — not an operator — drove the revoke.
    pub fn manager_revoke_permission_from_subordinate_as_agent(
        &mut self,
        token_ref: &str,
        manager_id: &AgentId,
        target_id: &AgentId,
        permission: &Permission,
    ) -> Result<(), KernelError> {
        let result =
            self.manager_revoke_permission_from_subordinate(manager_id, target_id, permission);
        let namespace = self.agents.get(target_id).map(|a| a.namespace_id.clone());
        self.record_audit(
            "agent",
            manager_id.as_str(),
            "agent:token_authenticated_manager_revoke_permission",
            Some("agent"),
            Some(target_id.as_str()),
            namespace.as_ref(),
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Denied
            },
            serde_json::json!({
                "permission": permission.as_str(),
                "auth_source": "agent_token",
                "token_ref": token_ref,
                "trust_boundary": "a per-agent access token authenticated the manager directly (no operator in the loop); the manager-subtree authorization was NOT bypassed",
            }),
        );
        result
    }

    /// Record that the operator minted a per-agent access token for `agent_id`. The
    /// token store ([`crate::agent_auth`]) lives outside the kernel (an auth-layer
    /// concern, like operator sessions), but its lifecycle is recorded in the SAME
    /// durable audit log so an operator can see who minted/revoked agent credentials.
    /// Only the **public** `token_id` handle is recorded — never the raw token.
    pub fn audit_agent_token_minted(&mut self, operator: &str, agent_id: &AgentId, token_id: &str) {
        let namespace = self.agents.get(agent_id).map(|a| a.namespace_id.clone());
        self.record_audit(
            "operator",
            operator,
            "agent:mint_token",
            Some("agent"),
            Some(agent_id.as_str()),
            namespace.as_ref(),
            AuditResult::Success,
            serde_json::json!({ "token_id": token_id }),
        );
    }

    /// Record that the operator revoked a per-agent access token. `found` distinguishes
    /// a real revocation (Success) from a no-op revoke of an unknown token (Denied). Only
    /// the public `token_id` is recorded — never the raw token.
    pub fn audit_agent_token_revoked(
        &mut self,
        operator: &str,
        agent_id: &AgentId,
        token_id: &str,
        found: bool,
    ) {
        let namespace = self.agents.get(agent_id).map(|a| a.namespace_id.clone());
        self.record_audit(
            "operator",
            operator,
            "agent:revoke_token",
            Some("agent"),
            Some(agent_id.as_str()),
            namespace.as_ref(),
            if found {
                AuditResult::Success
            } else {
                AuditResult::Denied
            },
            serde_json::json!({ "token_id": token_id }),
        );
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

        // Bounded transient-retry pass (the honest "next-tick retry-ready" state,
        // `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §7). A failed run whose class is an
        // auto-retryable transient and whose `[2m,10m,30m,2h]` backoff has elapsed
        // is re-attempted here through the SAME governed `retry_run` path (which
        // re-checks the enabled runtime, binary on PATH, and permission, and stamps
        // the `retried_from` lineage so the next attempt's backoff grows). There is
        // no background scheduler: eligibility is checked against real wall time
        // only when an operator/cron invokes a tick. Bounded by the per-tick cap.
        let ready = self.transient_retry_ready(real_now_secs());
        for run_id in ready.into_iter().take(max_tasks) {
            let namespace = self
                .runs
                .get(&run_id)
                .and_then(|r| self.tasks.get(&r.task_id))
                .map(|t| t.namespace_id.clone());
            match self.retry_run(&run_id) {
                Ok(new_run_id) => {
                    result.transient_retries += 1;
                    result.actions_taken += 1;
                    self.record_audit(
                        "kernel",
                        "prime",
                        "autonomy:transient_retry",
                        Some("run"),
                        Some(new_run_id.as_str()),
                        namespace.as_ref(),
                        AuditResult::Success,
                        serde_json::json!({ "retried_from": run_id.as_str() }),
                    );
                }
                Err(e) => {
                    // A re-attempt that fails again is honest, not fatal to the
                    // tick: record why and move on (the new run carries its own
                    // class + next backoff, or is exhausted).
                    let reason = format!("Transient retry of run {run_id} failed: {e}");
                    result.skipped_reasons.push(reason.clone());
                    self.record_audit(
                        "kernel",
                        "prime",
                        "autonomy:transient_retry_failed",
                        Some("run"),
                        Some(run_id.as_str()),
                        namespace.as_ref(),
                        AuditResult::Failed,
                        serde_json::json!({ "reason": reason }),
                    );
                }
            }
        }

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

    /// Resolve a candidate orchestration id against the live records for a RUN request,
    /// returning the canonical id ONLY when it names an EXISTING orchestration that has at
    /// least one PENDING brief left to run. Mirrors openclaw's `resolveControlledSubagentTarget`
    /// (`reference/openclaw-main/src/agents/subagent-control.ts`): a control action lands only
    /// on a target that exists AND is runnable. A non-existent id, or one whose briefs are all
    /// terminal, returns `None` (the caller fails closed — an honest reply, never a faked run).
    pub fn runnable_orchestration_id(&self, candidate: &str) -> Option<String> {
        let oid = OrchestrationId::new(candidate.to_string());
        let o = self.orchestrations.get(&oid)?;
        let has_pending = o
            .steps
            .iter()
            .any(|s| s.outcome == relux_core::StepOutcome::Pending);
        if has_pending {
            Some(o.id.0.clone())
        } else {
            None
        }
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
        // Enforcement uses `authorizes` (not `matches_exact`) so a scoped grant
        // (`tool:<plugin>:*`) covers the concrete tool perms in that plugin; exact
        // grants still match exactly. See `relux_core::Permission::authorizes`.
        for required_perm in &required_permissions {
            if !agent.permissions.iter().any(|p| p.authorizes(required_perm)) {
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
            resumed_from: None,
            session: None,
            artifacts: Vec::new(),
            proposed_changes: Vec::new(),
            failure_class: None,
            retry: None,
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

        // A tool whose declared approval blocks a direct invocation (a non-low-risk
        // operator-configured tool) is refused here - it is never run just because a
        // runtime is enabled - UNLESS a standing allow-always grant covers this exact
        // (subject, plugin, tool, permission, risk). The grant bypasses ONLY this
        // prompt; the permission check above and the runtime gate below still apply.
        if self.tool_needs_approval(plugin_id, tool_name) {
            match self.matching_persistent_grant_id(agent_id, plugin_id, tool_name) {
                Some(grant_id) => {
                    self.record_persistent_grant_use(
                        &grant_id, agent_id, plugin_id, tool_name, &namespace, &required,
                        Some(run_id),
                    );
                }
                None => {
                    self.push_run_event(
                        run_id,
                        "tool_call_denied",
                        "kernel",
                        &format!("denied {tool_name}: requires approval"),
                        serde_json::json!({ "tool": tool_name, "plugin": plugin_id.as_str() }),
                    );
                    self.record_audit(
                        "agent",
                        agent_id.as_str(),
                        required.as_str(),
                        Some("tool"),
                        Some(tool_name),
                        Some(&namespace),
                        AuditResult::Denied,
                        serde_json::json!({ "run": run_id.as_str(), "reason": "requires approval" }),
                    );
                    return Err(KernelError::ToolRequiresApproval {
                        plugin: plugin_id.to_string(),
                        tool: tool_name.to_string(),
                    });
                }
            }
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

        // Refuse a tool whose declared approval blocks a direct invocation (a
        // non-low-risk operator-configured tool); audited as a denial - UNLESS a
        // standing allow-always grant covers this exact (subject, plugin, tool,
        // permission, risk). The grant bypasses ONLY this prompt; the permission
        // check above and the runtime gate below still apply.
        if self.tool_needs_approval(plugin_id, tool_name) {
            match self.matching_persistent_grant_id(agent_id, plugin_id, tool_name) {
                Some(grant_id) => {
                    self.record_persistent_grant_use(
                        &grant_id, agent_id, plugin_id, tool_name, &namespace, &required, None,
                    );
                }
                None => {
                    self.record_audit(
                        "agent",
                        agent_id.as_str(),
                        required.as_str(),
                        Some("tool"),
                        Some(tool_name),
                        Some(&namespace),
                        AuditResult::Denied,
                        serde_json::json!({
                            "via": "invoke",
                            "plugin": plugin_id.as_str(),
                            "reason": "requires approval"
                        }),
                    );
                    return Err(KernelError::ToolRequiresApproval {
                        plugin: plugin_id.to_string(),
                        tool: tool_name.to_string(),
                    });
                }
            }
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
                    // A tool whose declared approval blocks a direct invocation
                    // (any non-low-risk operator-configured tool) is NOT runnable
                    // just because a runtime is enabled - it is honestly gated.
                    // Bundled tools all declare `Never`, so this never changes them.
                    if approval_blocks_direct_invocation(&tool.approval, &tool.risk) {
                        ToolExecutability::NeedsApproval
                    } else {
                        match agent_for_permission {
                            Some(agent_id)
                                if !self.agent_holds_permission(agent_id, &tool.permission) =>
                            {
                                ToolExecutability::MissingPermission
                            }
                            _ => ToolExecutability::Ready,
                        }
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
        // MCP tool (`mcp:<server>`): the required permission is the derived
        // `tool:mcp-<server>:<verb>`. The server must be registered and the tool name
        // must be a safe identifier; resolution never touches the plugin manifest map.
        if plugin_id.as_str().starts_with("mcp:") {
            let server = self.mcp_server_for_plugin(plugin_id).ok_or_else(|| {
                KernelError::UnknownMcpServer(
                    plugin_id
                        .as_str()
                        .strip_prefix("mcp:")
                        .unwrap_or(plugin_id.as_str())
                        .to_string(),
                )
            })?;
            if !relux_core::is_valid_mcp_tool_name(tool_name) {
                return Err(KernelError::InvalidMcpToolName {
                    server: server.id.clone(),
                    tool: tool_name.to_string(),
                });
            }
            let required = Permission::new(relux_core::mcp_tool_permission(&server.id, tool_name))
                .map_err(|e| KernelError::InvalidMcpToolName {
                    server: server.id.clone(),
                    tool: format!("{tool_name}: {e}"),
                })?;
            return Ok((namespace, required));
        }
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

    /// Whether an installed tool's declared approval requirement blocks a direct
    /// (no-approval-flow) invocation. The kernel does not yet wire a per-tool-call
    /// approval flow, so a tool that requires approval is honestly refused here
    /// rather than run. An unknown plugin/tool returns `false` (the caller's own
    /// resolution already failed it). Bundled tools declare `Never`, so this never
    /// changes their behavior.
    fn tool_needs_approval(&self, plugin_id: &PluginId, tool_name: &str) -> bool {
        // MCP tool: gate on the operator's classification (default Medium + Required
        // → always gated until classified).
        if let Some(server) = self.mcp_server_for_plugin(plugin_id) {
            let c = server.tool_classification(tool_name);
            return approval_blocks_direct_invocation(&c.approval, &c.risk);
        }
        self.plugins
            .get(plugin_id)
            .and_then(|m| m.capabilities.tools.iter().find(|t| t.name == tool_name))
            .map(|t| approval_blocks_direct_invocation(&t.approval, &t.risk))
            .unwrap_or(false)
    }

    /// True when `agent_id` exists and holds a grant that AUTHORIZES `permission` — an
    /// exact grant, or a scoped tool-plugin wildcard (`tool:<plugin>:*`) covering it.
    /// This is the single chokepoint every tool-invocation permission check routes
    /// through (`relux_core::Permission::authorizes`). Revoke/grant bookkeeping still
    /// uses exact match, so a scoped grant is one explicit, individually-revocable row.
    fn agent_holds_permission(&self, agent_id: &AgentId, permission: &Permission) -> bool {
        self.agents
            .get(agent_id)
            .map(|a| a.permissions.iter().any(|p| p.authorizes(permission)))
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
        // MCP tool (`mcp:<server>`): route to the loopback MCP `tools/call` client.
        // The server must exist + be enabled; the tool name is re-validated (defense
        // in depth); the loopback endpoint is re-validated on every call inside the
        // client. The result is the SHAPED, sanitized value — never the raw MCP
        // envelope. Arbitrary downloaded code is never run — only the operator's
        // own loopback MCP server is dialed.
        if let Some(server) = self.mcp_server_for_plugin(plugin_id) {
            if !server.enabled {
                return Err(KernelError::McpServerDisabled(server.id.clone()));
            }
            if !relux_core::is_valid_mcp_tool_name(tool_name) {
                return Err(KernelError::InvalidMcpToolName {
                    server: server.id.clone(),
                    tool: tool_name.to_string(),
                });
            }
            return crate::mcp::call_tool(&server.endpoint, tool_name, input, server.timeout_ms)
                .map_err(|e| KernelError::ToolRuntimeInvocation {
                    plugin: plugin_id.to_string(),
                    tool: tool_name.to_string(),
                    message: e.to_string(),
                });
        }
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

    /// Persist the bounded, redacted **session identity / handoff** an adapter
    /// declared in its structured result envelope onto the durable run record
    /// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §3). The raw `session_id` is sanitized,
    /// bounded, and tagged with the adapter source and a per-kind `resume_supported`
    /// capability by [`relux_core::RunSession::from_envelope`]. Best-effort: an
    /// unknown run id, or an envelope that declared no (safe) session id, is a no-op
    /// (we never write an empty session and never fabricate one). Stores only the
    /// session id, source, and capability — never a token, raw envelope, or full log.
    fn set_run_session(
        &mut self,
        run_id: &RunId,
        session_id: Option<&str>,
        kind: relux_core::AdapterKind,
    ) {
        let Some(session) = relux_core::RunSession::from_envelope(session_id, &kind) else {
            return;
        };
        if let Some(run) = self.runs.get_mut(run_id) {
            run.session = Some(session);
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
            // The retry lineage is now linked, so the new run's transient-retry
            // attempt index (its `retried_from` depth) is finally correct. If this
            // re-attempt ALSO failed transiently, re-plan its bounded-retry state
            // against that attempt so the backoff grows across attempts and the
            // budget can exhaust (at fail time the lineage was not yet stamped, so
            // the attempt read as 0). Non-retryable failures are left untouched.
            let retry_class = self.runs.get(&new_run_id).and_then(|run| {
                if run.status == RunStatus::Failed {
                    run.failure_class.filter(|c| c.retryable())
                } else {
                    None
                }
            });
            if let Some(class) = retry_class {
                let attempt = self.transient_attempt_for(&new_run_id);
                let replanned = RunRetryState::plan(class, attempt, real_now_secs());
                if let Some(run) = self.runs.get_mut(&new_run_id) {
                    run.retry = replanned;
                }
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

    /// Resume a prior run's provider session through the governed adapter gate
    /// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §3 — OpenClaw `resumeSessionId` /
    /// `runCliWithSession`). **Distinct from [`Self::retry_run`]**: a retry is a
    /// cold fresh run on the same task; a resume *continues* the recorded adapter
    /// session by threading its captured `session.adapter_session_id` into
    /// `--resume` on the same non-bypass spawn.
    ///
    /// Honest by construction — it never fakes a continuation. The prior run must be
    /// terminal (completed or failed) and carry a session whose adapter
    /// `resume_supported` is set ([`relux_core::plan_resume`] is the single source of
    /// truth). Otherwise it refuses with [`KernelError::RunResumeNotSupported`] and a
    /// specific, secret-free reason (no captured session, an adapter without safe
    /// non-interactive resume, or a run still in flight) — the operator re-runs fresh
    /// (a distinct action). When supported, it dispatches through the SAME governed
    /// CLI path a normal run uses (enabled runtime + PATH probe + permission gate +
    /// bounded, non-bypass spawn), creating a new run stamped `resumed_from`. If the
    /// upstream session is invalid/expired the adapter rejects it and the run fails
    /// honestly — never a fabricated success.
    pub fn resume_run(&mut self, run_id: &RunId) -> Result<RunId, KernelError> {
        // 1. Read the prior run's terminal state + captured session, then let the
        //    pure core decision say whether a resume is honestly possible.
        let (task_id, status, session) = {
            let run = self
                .runs
                .get(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            (run.task_id.clone(), run.status.clone(), run.session.clone())
        };
        let terminal = matches!(status, RunStatus::Completed | RunStatus::Failed);
        let adapter_session_id = match relux_core::plan_resume(session.as_ref(), terminal) {
            relux_core::ResumeDisposition::Supported { adapter_session_id, .. } => adapter_session_id,
            relux_core::ResumeDisposition::NotSupported { reason } => {
                return Err(KernelError::RunResumeNotSupported {
                    run: run_id.to_string(),
                    reason,
                });
            }
        };
        // Supported implies a session is present.
        let session = session.expect("a supported resume carries a session");

        // 2. Same bar as a retry: the task must still exist and be assigned.
        let (namespace, adapter) = {
            let task = self
                .tasks
                .get(&task_id)
                .ok_or_else(|| KernelError::UnknownTask(task_id.to_string()))?;
            let agent_id = task
                .assigned_agent
                .clone()
                .ok_or_else(|| KernelError::TaskNotAssigned(task_id.to_string()))?;
            let adapter = self
                .agents
                .get(&agent_id)
                .ok_or_else(|| KernelError::UnknownAgent(agent_id.to_string()))?
                .adapter_plugin
                .clone();
            (task.namespace_id.clone(), adapter)
        };

        // 3. Re-queue the task and start the fresh run that will carry the resume.
        if let Some(task) = self.tasks.get_mut(&task_id) {
            task.status = TaskStatus::Queued;
            task.updated_at = self.clock.tick();
        }
        let new_run_id = self.start_run(&task_id)?;

        // 4. Stamp the new run as a resume BEFORE dispatch: record its lineage and
        //    carry the prior session forward, so `prepare_cli_run` threads
        //    `--resume <session_id>` (a normal run has no `resumed_from`, so resume
        //    never leaks onto a cold run). A successful resume's own envelope will
        //    overwrite `session` with the freshest reported one in `finalize_cli_run`.
        if let Some(run) = self.runs.get_mut(&new_run_id) {
            run.resumed_from = Some(run_id.clone());
            run.session = Some(session);
        }
        self.push_run_event(
            &new_run_id,
            "run_resumed_from",
            "kernel",
            &format!("resume of run {run_id}"),
            serde_json::json!({
                "resumed_from": run_id.as_str(),
                "adapter_session_id": adapter_session_id,
            }),
        );

        // 5. Dispatch through the governed CLI path (a resumable session is a Claude
        //    CLI session, which runs via the CLI path). Capture the outcome so the
        //    lineage is durable even when this attempt also fails honestly.
        let result = self.execute_cli_run(&task_id, &adapter);
        self.record_audit(
            "user",
            "founder",
            "run:resume",
            Some("run"),
            Some(new_run_id.as_str()),
            Some(&namespace),
            if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failed
            },
            serde_json::json!({ "resumed_from": run_id.as_str(), "task": task_id.as_str() }),
        );
        result.map(|_| new_run_id)
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
        // A rejected per-tool-call approval drops its bound invocation outright, so
        // a rejected call can never be executed (the execute path also fails closed
        // on a non-`Approved` status; this just frees the binding eagerly).
        if !approve {
            self.pending_tool_invocations.remove(id);
        }
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

    /// The per-tool-call binding for an approval, if it is a tool-invocation
    /// approval (read-only; used to render the Approvals page detail + Execute
    /// affordance).
    pub fn pending_tool_invocation(&self, id: &ApprovalId) -> Option<&PendingToolInvocation> {
        self.pending_tool_invocations.get(id)
    }

    // --- Per-tool-call approval flow --------------------------------------

    /// Request a per-call approval to invoke ONE non-low-risk configured tool with
    /// a specific set of arguments (`docs/RELUX_MASTER_PLAN.md` §7.4 per-call
    /// approval, `docs/reference-driven-development.md` "per-tool-call approval").
    ///
    /// This is the two-phase "register the approval before anything can run" shape
    /// from openclaw's `registerExecApprovalRequest`
    /// (`reference/openclaw-main/src/agents/bash-tools.exec-approval-request.ts`):
    /// a [`PendingToolInvocation`] is registered, bound to the exact `(plugin, tool,
    /// agent, args snapshot + SHA-256)`, alongside a generic [`Approval`] the
    /// operator decides on. Nothing is executed here.
    ///
    /// Fail-closed gates, in order: the tool must exist and the subject agent must
    /// hold its permission; the tool must ACTUALLY require approval (a directly
    /// runnable tool is refused — the caller should just invoke it); and the args
    /// must be within [`MAX_TOOL_INVOCATION_ARGS_BYTES`]. The args are stored
    /// verbatim for later execution, with a secret-redacted preview for display.
    pub fn request_tool_invocation_approval(
        &mut self,
        requested_by: &str,
        agent_id: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<ApprovalId, KernelError> {
        let (namespace, required) = self.resolve_tool_permission(agent_id, plugin_id, tool_name)?;

        if !self.agent_holds_permission(agent_id, &required) {
            self.record_audit(
                "user",
                requested_by,
                required.as_str(),
                Some("tool"),
                Some(tool_name),
                Some(&namespace),
                AuditResult::Denied,
                serde_json::json!({
                    "via": "tool_invocation:request",
                    "plugin": plugin_id.as_str(),
                    "subject_agent": agent_id.as_str()
                }),
            );
            return Err(KernelError::PermissionDenied {
                agent: agent_id.to_string(),
                permission: required.to_string(),
            });
        }

        // Only a tool whose declared approval blocks a direct invocation is eligible
        // for the per-call flow. A directly-runnable (low-risk auto-approve) tool is
        // refused: the operator should invoke it, not gate it.
        if !self.tool_needs_approval(plugin_id, tool_name) {
            return Err(KernelError::ToolDoesNotRequireApproval {
                plugin: plugin_id.to_string(),
                tool: tool_name.to_string(),
            });
        }

        // Bound the args to what the loopback runtime would itself accept.
        let body =
            serde_json::to_vec(&input).map_err(|e| KernelError::InvalidToolDefinition {
                plugin: plugin_id.to_string(),
                message: format!("arguments are not serializable JSON: {e}"),
            })?;
        if body.len() > MAX_TOOL_INVOCATION_ARGS_BYTES {
            return Err(KernelError::ToolInvocationArgsTooLarge {
                plugin: plugin_id.to_string(),
                tool: tool_name.to_string(),
                size: body.len(),
                max: MAX_TOOL_INVOCATION_ARGS_BYTES,
            });
        }

        let risk = self.tool_risk_for(plugin_id, tool_name);
        let args_sha256 = sha256_hex(&body);
        let args_preview = redact_args_for_preview(&input);

        // Register the generic approval (Pending, audited) that the operator decides.
        let action = format!("invoke tool {tool_name} on {plugin_id} (as {agent_id})");
        let reason = format!(
            "{risk:?}-risk per-call tool invocation requested by {requested_by}; args sha256 {short}…",
            short = &args_sha256[..args_sha256.len().min(12)]
        );
        let id = self.request_approval(requested_by, &action, &reason, risk.clone(), Some(&namespace));
        // Bind the invocation with the same timestamp the approval recorded, so the
        // two read consistently on the Approvals page.
        let created_at = self
            .approvals
            .get(&id)
            .map(|a| a.created_at.clone())
            .unwrap_or_default();

        self.pending_tool_invocations.insert(
            id.clone(),
            PendingToolInvocation {
                approval_id: id.clone(),
                plugin_id: plugin_id.clone(),
                tool_name: tool_name.to_string(),
                agent_id: agent_id.clone(),
                permission: required.to_string(),
                input,
                args_sha256: args_sha256.clone(),
                args_preview,
                requested_by: requested_by.to_string(),
                risk,
                created_at,
                consumed: false,
            },
        );

        self.record_audit(
            "user",
            requested_by,
            "tool_invocation:request",
            Some("approval"),
            Some(id.as_str()),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({
                "plugin": plugin_id.as_str(),
                "tool": tool_name,
                "subject_agent": agent_id.as_str(),
                "args_sha256": args_sha256
            }),
        );
        Ok(id)
    }

    /// Execute the single invocation bound to an APPROVED per-call approval, exactly
    /// once (`docs/RELUX_MASTER_PLAN.md` §7.4 per-call approval). This is the
    /// consume-once counterpart of openclaw's
    /// `consumeExecApprovalFollowupRuntimeHandoff`: the binding is run only when the
    /// approval is `Approved` and the binding has not been consumed, every bound
    /// field is re-validated, and the binding is marked consumed on a single attempt.
    ///
    /// Defense in depth: the approval must be `Approved`; the binding must exist and
    /// be unconsumed; the tool must still exist and the subject agent must still hold
    /// its permission; the stored args must still hash to the recorded SHA-256. The
    /// stored snapshot — never client-supplied args — is executed, so an approved
    /// call cannot be modified before it runs. A single attempt (success OR runtime
    /// failure) consumes the binding; it can never run again without a new approval.
    pub fn execute_approved_tool_invocation(
        &mut self,
        id: &ApprovalId,
        executed_by: &str,
    ) -> Result<ToolInvocationResult, KernelError> {
        // 1. The approval must exist and be Approved.
        let approval = self
            .approvals
            .get(id)
            .ok_or_else(|| KernelError::UnknownApproval(id.to_string()))?;
        if approval.status != ApprovalStatus::Approved {
            return Err(KernelError::ToolInvocationNotApproved {
                id: id.to_string(),
                status: approval_status_label(&approval.status),
            });
        }

        // 2. The binding must exist and be unconsumed.
        let binding = self
            .pending_tool_invocations
            .get(id)
            .ok_or_else(|| KernelError::NoBoundToolInvocation(id.to_string()))?;
        if binding.consumed {
            return Err(KernelError::ToolInvocationConsumed(id.to_string()));
        }
        let plugin_id = binding.plugin_id.clone();
        let tool_name = binding.tool_name.clone();
        let agent_id = binding.agent_id.clone();
        let input = binding.input.clone();
        let stored_hash = binding.args_sha256.clone();

        // 3. Integrity: the stored snapshot must still hash to the recorded digest
        //    (defense in depth against a tampered persisted binding).
        let body =
            serde_json::to_vec(&input).map_err(|e| KernelError::InvalidToolDefinition {
                plugin: plugin_id.to_string(),
                message: format!("stored arguments are not serializable JSON: {e}"),
            })?;
        if sha256_hex(&body) != stored_hash {
            return Err(KernelError::ToolInvocationArgsTampered(id.to_string()));
        }

        // 4. Re-validate the tool still exists and the subject still holds the
        //    permission (a permission could have been revoked since approval).
        let (namespace, required) =
            self.resolve_tool_permission(&agent_id, &plugin_id, &tool_name)?;
        if !self.agent_holds_permission(&agent_id, &required) {
            self.record_audit(
                "user",
                executed_by,
                required.as_str(),
                Some("tool"),
                Some(&tool_name),
                Some(&namespace),
                AuditResult::Denied,
                serde_json::json!({
                    "via": "tool_invocation:execute",
                    "approval": id.as_str(),
                    "plugin": plugin_id.as_str(),
                    "subject_agent": agent_id.as_str()
                }),
            );
            return Err(KernelError::PermissionDenied {
                agent: agent_id.to_string(),
                permission: required.to_string(),
            });
        }

        // 5. Consume the binding on this single attempt — BEFORE dispatch — so that
        //    even a runtime failure cannot be retried without a fresh approval.
        if let Some(b) = self.pending_tool_invocations.get_mut(id) {
            b.consumed = true;
        }

        // 6. Execute via the same runtime as a direct invoke, bypassing the
        //    needs-approval gate (this IS the granted approval). Honest failures.
        let output = match self.execute_tool_runtime(&plugin_id, &tool_name, &input) {
            Ok(output) => output,
            Err(e) => {
                self.record_audit(
                    "user",
                    executed_by,
                    required.as_str(),
                    Some("tool"),
                    Some(&tool_name),
                    Some(&namespace),
                    AuditResult::Failed,
                    serde_json::json!({
                        "via": "tool_invocation:execute",
                        "approval": id.as_str(),
                        "plugin": plugin_id.as_str(),
                        "reason": e.to_string()
                    }),
                );
                return Err(e);
            }
        };

        self.record_audit(
            "user",
            executed_by,
            required.as_str(),
            Some("tool"),
            Some(&tool_name),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({
                "via": "tool_invocation:execute",
                "approval": id.as_str(),
                "plugin": plugin_id.as_str(),
                "subject_agent": agent_id.as_str()
            }),
        );
        Ok(ToolInvocationResult {
            plugin_id: plugin_id.to_string(),
            tool_name: tool_name.to_string(),
            agent_id: agent_id.to_string(),
            permission: required.to_string(),
            output,
        })
    }

    // --- Persistent allow-always grants -----------------------------------

    /// The id of a standing allow-always grant that authorizes a direct invocation
    /// of `tool_name` on `plugin_id` as `subject`, or `None`. The tool's CURRENT
    /// required permission and risk are looked up and the grant must match them
    /// EXACTLY (`PersistentGrant::authorizes_invocation`), so a tool whose permission
    /// changed or whose risk escalated since the grant was created no longer matches
    /// (fail closed → the per-call approval is required again). An unknown plugin/tool
    /// matches nothing. This is the read-only half of openclaw's `hasDurableExecApproval`.
    fn matching_persistent_grant_id(
        &self,
        subject: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
    ) -> Option<String> {
        // MCP tool: the grant must match the derived permission + the operator's
        // classified risk EXACTLY, same fail-closed re-check as a plugin tool.
        if let Some(server) = self.mcp_server_for_plugin(plugin_id) {
            let permission = relux_core::mcp_tool_permission(&server.id, tool_name);
            let risk = server.tool_classification(tool_name).risk;
            return self
                .persistent_grants
                .values()
                .find(|g| {
                    g.authorizes_invocation(
                        subject,
                        plugin_id.as_str(),
                        tool_name,
                        &permission,
                        &risk,
                    )
                })
                .map(|g| g.id.clone());
        }
        let tool = self
            .plugins
            .get(plugin_id)
            .and_then(|m| m.capabilities.tools.iter().find(|t| t.name == tool_name))?;
        let permission = tool.permission.as_str();
        let risk = &tool.risk;
        self.persistent_grants
            .values()
            .find(|g| {
                g.authorizes_invocation(subject, plugin_id.as_str(), tool_name, permission, risk)
            })
            .map(|g| g.id.clone())
    }

    /// Stamp a grant's `last_used_at` and audit the bypass when a standing
    /// allow-always grant let an invocation through the per-call gate. This is the
    /// counterpart of openclaw's `recordAllowlistUse` (record that a durable
    /// approval was used) — the use of a persistent grant is itself an audit event.
    #[allow(clippy::too_many_arguments)]
    fn record_persistent_grant_use(
        &mut self,
        grant_id: &str,
        agent_id: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
        namespace: &NamespaceId,
        required: &Permission,
        run_id: Option<&RunId>,
    ) {
        let used_at = self.clock.tick();
        if let Some(g) = self.persistent_grants.get_mut(grant_id) {
            g.last_used_at = Some(used_at);
        }
        self.record_audit(
            "agent",
            agent_id.as_str(),
            "grant:use",
            Some("grant"),
            Some(grant_id),
            Some(namespace),
            AuditResult::Success,
            serde_json::json!({
                "grant": grant_id,
                "plugin": plugin_id.as_str(),
                "tool": tool_name,
                "subject_agent": agent_id.as_str(),
                "permission": required.as_str(),
                "run": run_id.map(|r| r.as_str()),
            }),
        );
    }

    /// Create a persistent allow-always grant so a FUTURE matching invocation of
    /// `tool_name` on `plugin_id` as `subject_agent` bypasses the per-call approval
    /// prompt (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §5 P2). This is the durable
    /// counterpart of [`request_tool_invocation_approval`](Self::request_tool_invocation_approval):
    /// it records a standing decision instead of a one-shot binding.
    ///
    /// Fail-closed gates, in order (mirroring the per-call request path so a grant can
    /// never widen the trust boundary): the tool must exist and the subject must hold
    /// its permission; and the tool must ACTUALLY require approval (a directly-runnable
    /// low-risk tool is refused — a grant would be meaningless). The grant snapshots the
    /// tool's CURRENT permission + risk, which matching later re-checks exactly. Creating
    /// an identical grant is idempotent (the existing row is returned, no duplicate).
    pub fn grant_persistent_tool_invocation(
        &mut self,
        created_by: &str,
        subject_agent: &AgentId,
        plugin_id: &PluginId,
        tool_name: &str,
    ) -> Result<PersistentGrant, KernelError> {
        let (namespace, required) =
            self.resolve_tool_permission(subject_agent, plugin_id, tool_name)?;

        if !self.agent_holds_permission(subject_agent, &required) {
            self.record_audit(
                "user",
                created_by,
                required.as_str(),
                Some("tool"),
                Some(tool_name),
                Some(&namespace),
                AuditResult::Denied,
                serde_json::json!({
                    "via": "grant:create",
                    "plugin": plugin_id.as_str(),
                    "subject_agent": subject_agent.as_str()
                }),
            );
            return Err(KernelError::PermissionDenied {
                agent: subject_agent.to_string(),
                permission: required.to_string(),
            });
        }

        // Only a tool that genuinely gates is grantable. A directly-runnable
        // (low-risk auto-approve) tool needs no grant; refusing it keeps allow-always
        // honest about what it covers (openclaw only persists allow-always for the
        // safe-to-persist case).
        if !self.tool_needs_approval(plugin_id, tool_name) {
            return Err(KernelError::ToolDoesNotRequireApproval {
                plugin: plugin_id.to_string(),
                tool: tool_name.to_string(),
            });
        }

        let risk = self.tool_risk_for(plugin_id, tool_name);

        // Idempotent: an identical standing grant already covers this exact
        // invocation — return it rather than minting a duplicate row.
        if let Some(existing) = self.persistent_grants.values().find(|g| {
            g.authorizes_invocation(
                subject_agent,
                plugin_id.as_str(),
                tool_name,
                required.as_str(),
                &risk,
            )
        }) {
            return Ok(existing.clone());
        }

        self.next_grant += 1;
        let id = format!("grant_{:04}", self.next_grant);
        let created_at = self.clock.tick();
        let grant = PersistentGrant {
            id: id.clone(),
            created_by: created_by.to_string(),
            subject_agent: subject_agent.clone(),
            plugin_id: plugin_id.as_str().to_string(),
            tool_name: tool_name.to_string(),
            permission: required.as_str().to_string(),
            risk: risk.clone(),
            created_at,
            last_used_at: None,
        };
        self.persistent_grants.insert(id.clone(), grant.clone());
        self.record_audit(
            "user",
            created_by,
            "grant:create",
            Some("grant"),
            Some(&id),
            Some(&namespace),
            AuditResult::Success,
            serde_json::json!({
                "plugin": plugin_id.as_str(),
                "tool": tool_name,
                "subject_agent": subject_agent.as_str(),
                "permission": required.as_str(),
                "risk": format!("{:?}", risk),
            }),
        );
        Ok(grant)
    }

    /// Revoke (remove) a persistent allow-always grant by id, audited. After this
    /// the formerly-covered invocation requires per-call approval again — the grant
    /// stays revocable as a single, explicit row (openclaw's allowlist entries are
    /// individually identified + removable). Errors on an unknown grant id.
    pub fn revoke_persistent_grant(
        &mut self,
        id: &str,
        revoked_by: &str,
    ) -> Result<(), KernelError> {
        let grant = self
            .persistent_grants
            .remove(id)
            .ok_or_else(|| KernelError::UnknownPersistentGrant(id.to_string()))?;
        let namespace = self
            .agents
            .get(&grant.subject_agent)
            .map(|a| a.namespace_id.clone());
        self.record_audit(
            "user",
            revoked_by,
            "grant:revoke",
            Some("grant"),
            Some(id),
            namespace.as_ref(),
            AuditResult::Success,
            serde_json::json!({
                "plugin": grant.plugin_id,
                "tool": grant.tool_name,
                "subject_agent": grant.subject_agent.as_str(),
            }),
        );
        Ok(())
    }

    /// All persistent grants, sorted by id (read-only; for the Approvals/Governance UI).
    pub fn persistent_grants(&self) -> Vec<&PersistentGrant> {
        let mut out: Vec<&PersistentGrant> = self.persistent_grants.values().collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// A single persistent grant by id, if present.
    pub fn persistent_grant(&self, id: &str) -> Option<&PersistentGrant> {
        self.persistent_grants.get(id)
    }

    /// "Allow always" on a PENDING per-tool-call approval: create a standing grant
    /// from the approval's bound invocation AND approve that pending approval (so the
    /// current bound call can still run once). This is the openclaw `allow-always`
    /// decision: it approves the in-flight request AND persists a durable grant.
    ///
    /// The grant is created FIRST (re-validating the subject still holds the permission
    /// and the tool still gates), so if it fails nothing is approved and the operator
    /// can fall back to "approve once". The approval must carry a tool-invocation
    /// binding; a generic approval has nothing to grant.
    pub fn allow_always_from_approval(
        &mut self,
        id: &ApprovalId,
        approver: &str,
    ) -> Result<PersistentGrant, KernelError> {
        let binding = self
            .pending_tool_invocations
            .get(id)
            .ok_or_else(|| KernelError::NoBoundToolInvocation(id.to_string()))?;
        let subject = binding.agent_id.clone();
        let plugin_id = binding.plugin_id.clone();
        let tool_name = binding.tool_name.clone();

        let grant = self.grant_persistent_tool_invocation(
            approver,
            &subject,
            &plugin_id,
            &tool_name,
        )?;
        // Approve the in-flight pending approval too, so the bound one-shot can run.
        self.resolve_approval(id, true, approver, None)?;
        Ok(grant)
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
            agent_skills: self
                .agents
                .values()
                .filter(|a| !a.skills.is_empty())
                .map(|a| (a.id.0.clone(), a.skills.clone()))
                .collect(),
            all_task_ids: self.tasks.keys().map(|id| id.0.clone()).collect(),
            queued,
            recent,
        }
    }

    /// Take an owned, bounded read-only snapshot of the control-plane state the governed
    /// read-only context tools ([`crate::prime_tools`]) read from. Built ONCE under the kernel
    /// lock so the (slow) brain rounds of the tool loop run OUTSIDE the lock and the executors
    /// stay pure over the snapshot.
    ///
    /// Mirrors [`Self::inspect_state`]'s grounding view (the whole board, sorted by id for a
    /// deterministic order), projected to the compact views Prime speaks about. Bounded by
    /// [`MAX_SNAPSHOT_ITEMS`] per collection so a large board cannot blow up the clone; the list
    /// tools further bound what they render. Reads only — it mutates nothing and fabricates
    /// nothing.
    pub fn context_snapshot(&self, _ctx: &PrimeContext) -> crate::prime_tools::ContextSnapshot {
        let summary = self.inspect_state();

        let mut tasks: Vec<&Task> = self.tasks.values().collect();
        tasks.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        let tasks: Vec<crate::prime_tools::TaskView> = tasks
            .into_iter()
            .take(MAX_SNAPSHOT_ITEMS)
            .map(|t| crate::prime_tools::TaskView {
                id: t.id.0.clone(),
                title: t.title.clone(),
                status: t.status.clone(),
                assignee: t.assigned_agent.as_ref().map(|a| a.0.clone()),
                priority: t.priority,
                detail: task_detail_line(&t.input),
            })
            .collect();

        let mut agents: Vec<&Agent> = self.agents.values().collect();
        agents.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        let agents: Vec<crate::prime_tools::AgentView> = agents
            .into_iter()
            .take(MAX_SNAPSHOT_ITEMS)
            .map(|a| crate::prime_tools::AgentView {
                id: a.id.0.clone(),
                name: a.name.clone(),
                role: a.description.clone(),
                adapter: a.adapter_plugin.0.clone(),
                persona: a.persona.clone(),
            })
            .collect();

        let mut runs: Vec<&Run> = self.runs.values().collect();
        runs.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        let runs: Vec<crate::prime_tools::RunView> = runs
            .into_iter()
            .rev()
            .take(MAX_SNAPSHOT_ITEMS)
            .map(|r| crate::prime_tools::RunView {
                id: r.id.0.clone(),
                task_id: r.task_id.0.clone(),
                agent_id: r.agent_id.0.clone(),
                status: run_status_label(&r.status),
                adapter: r.adapter_plugin.0.clone(),
                started_at: r.started_at.clone(),
                ended_at: r.ended_at.clone(),
                duration_ms: r.duration_ms,
                // Redacted + bounded; the raw provider usage/cost envelope is never projected.
                summary: r.summary.as_deref().and_then(|s| redact_line(s, MAX_REDACTED_CHARS)),
                error: r.error.as_deref().and_then(|s| redact_line(s, MAX_REDACTED_CHARS)),
            })
            .collect();

        // Installed plugins/adapters, sorted by id (deterministic), with the tool count read from
        // the live manifest. The raw `source_label` (a local path / URL) is deliberately NOT
        // projected — only the source kind label.
        let plugins: Vec<crate::prime_tools::PluginView> = self
            .installed_plugins()
            .into_iter()
            .take(MAX_SNAPSHOT_ITEMS)
            .map(|p| crate::prime_tools::PluginView {
                id: p.id.0.clone(),
                version: p.version.clone(),
                kind: plugin_kind_label(&p.kind),
                enabled: p.enabled,
                protected: p.source_kind == PluginSourceKind::Bundled,
                source_kind: format!("{:?}", p.source_kind),
                tools: self
                    .plugins
                    .get(&p.id)
                    .map(|m| m.capabilities.tools.len())
                    .unwrap_or(0),
            })
            .collect();

        // Approvals, pending first then most-recent (the same ordering the HTTP list endpoint
        // uses), with the human-readable action/reason redacted + bounded. They carry no secret.
        let mut approval_refs: Vec<&Approval> = self.approvals.values().collect();
        approval_refs.sort_by(|a, b| {
            let pending = |ap: &Approval| matches!(ap.status, ApprovalStatus::Pending);
            pending(b)
                .cmp(&pending(a))
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        let approvals: Vec<crate::prime_tools::ApprovalView> = approval_refs
            .into_iter()
            .take(MAX_SNAPSHOT_ITEMS)
            .map(|a| crate::prime_tools::ApprovalView {
                id: a.id.0.clone(),
                status: approval_status_label(&a.status),
                risk: risk_level_label(&a.risk),
                requested_by: redact_line(&a.requested_by, MAX_ARG_REDACTED_CHARS)
                    .unwrap_or_default(),
                action: redact_line(&a.action, MAX_REDACTED_CHARS).unwrap_or_default(),
                reason: redact_line(&a.reason, MAX_REDACTED_CHARS).unwrap_or_default(),
            })
            .collect();

        crate::prime_tools::ContextSnapshot {
            summary,
            tasks,
            agents,
            runs,
            plugins,
            approvals,
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
    /// Run one Prime turn with the deterministic intent classifier.
    ///
    /// Convenience wrapper over [`Self::prime_turn_with_intent`] with no brain
    /// proposal, so the many call sites (CLI, autonomy, tests) keep the simple
    /// two-argument form and get exactly the deterministic behavior.
    pub fn prime_turn(
        &mut self,
        ctx: &PrimeContext,
        message: &str,
    ) -> Result<PrimeTurn, KernelError> {
        Ok(self.prime_turn_with_intent(ctx, message, None)?.0)
    }

    /// Run one Prime turn, optionally reconciling the deterministic intent with a
    /// brain-proposed intent.
    ///
    /// `intent_proposal` is a structured intent a configured brain proposed for
    /// this message (see [`crate::prime_intent`]). It is reconciled through the
    /// fail-closed [`crate::prime_intent::reconcile_intent`] gate: the brain can
    /// sharpen a misread intent, but can NEVER mint or run work from guarded chat,
    /// and can never silently auto-run a task. With `None` (no brain configured, or
    /// the brain failed/timed out) this is exactly the deterministic turn. Returns
    /// the turn plus where the final intent came from, for honest provenance and
    /// the audit log. Durable state changes still flow only through `decide` →
    /// [`Self::prime_execute`]; the brain authors no slots and runs no action.
    pub fn prime_turn_with_intent(
        &mut self,
        ctx: &PrimeContext,
        message: &str,
        intent_proposal: Option<&crate::prime_intent::BrainIntentProposal>,
    ) -> Result<(PrimeTurn, crate::prime_intent::IntentSource), KernelError> {
        self.prime_turn_with_intent_and_slots(ctx, message, intent_proposal, None)
    }

    /// Like [`Self::prime_turn_with_intent`], but additionally accepts an OPTIONAL
    /// brain-proposed set of task slots for a create turn.
    ///
    /// This is the chokepoint for brain-assisted slot extraction
    /// (`crates/relux-kernel/src/prime_slots.rs`): when the (already
    /// brain-reconciled, fail-closed-gated) plan is a create `Act`, a slot proposal
    /// is reconciled against the deterministic title and the live agent roster
    /// before the task is created. The brain authors no new work — it only sharpens
    /// a create the deterministic path already decided, and an unknown assignee /
    /// low-confidence / absent proposal leaves the deterministic slots exactly as
    /// they were. With `slot_proposal = None` this is byte-for-byte the deterministic
    /// turn. Durable state still flows only through `decide` →
    /// [`Self::prime_execute`].
    pub fn prime_turn_with_intent_and_slots(
        &mut self,
        ctx: &PrimeContext,
        message: &str,
        intent_proposal: Option<&crate::prime_intent::BrainIntentProposal>,
        slot_proposal: Option<&crate::prime_slots::BrainTaskSlots>,
    ) -> Result<(PrimeTurn, crate::prime_intent::IntentSource), KernelError> {
        self.prime_turn_with_brain(
            ctx,
            message,
            intent_proposal,
            BrainSlotProposals {
                task: slot_proposal,
                ..Default::default()
            },
        )
    }

    /// Like [`Self::prime_turn_with_intent_and_slots`], but accepts the full bundle of
    /// brain-proposed slots for whichever action the turn produces — task slots for a
    /// create, agent slots for an `AgentCreation`, and the advisory plugin/permission
    /// subject for a risky `Propose`. Each is validated at this single chokepoint
    /// before it can shape anything: a create is sharpened only after the fail-closed
    /// intent gate accepted it, an agent slot is rejected on a duplicate id / unknown
    /// adapter, and the plugin/permission subject only sharpens an action that STAYS
    /// gated behind a human approval. With an all-`None` bundle this is byte-for-byte
    /// the deterministic turn. Durable state still flows only through `decide` →
    /// [`Self::prime_execute`] (or, for risky actions, a human approval).
    pub fn prime_turn_with_brain(
        &mut self,
        ctx: &PrimeContext,
        message: &str,
        intent_proposal: Option<&crate::prime_intent::BrainIntentProposal>,
        slots: BrainSlotProposals<'_>,
    ) -> Result<(PrimeTurn, crate::prime_intent::IntentSource), KernelError> {
        // --- Multi-turn clarification memory (`docs/prime-processing-audit.md`) ----
        // Before classifying, see whether this message answers a clarifying question
        // Prime asked last turn. The deterministic resolver decides: a bare answer is
        // COMBINED with the stored original message (so the original request continues
        // through the same pipeline); a fresh standalone command/question supersedes the
        // pending context; an explicit cancellation drops it; a stale (expired) record is
        // ignored. When continuing, the brain proposals (computed on the raw answer, not
        // the combined message) are dropped and the deterministic combined classification
        // stands — the safe fallback that always exists.
        let key = Self::conversation_key(ctx);
        let now_secs = self.clock.secs();
        let mut effective_message: String = message.to_string();
        let mut continued = false;
        let mut intent_proposal = intent_proposal;
        let mut slots = slots;
        if let Some(pending) = self.pending_clarifications.get(&key).cloned() {
            match crate::prime_clarify_memory::resolve_pending(&pending, message, now_secs) {
                crate::prime_clarify_memory::ClarifyResolution::Cancelled => {
                    self.pending_clarifications.remove(&key);
                    let turn = self.clarification_cancelled_turn(ctx, &pending);
                    return Ok((turn, crate::prime_intent::IntentSource::Deterministic));
                }
                crate::prime_clarify_memory::ClarifyResolution::Expired
                | crate::prime_clarify_memory::ClarifyResolution::FreshRequest => {
                    // Drop the superseded/stale context; handle this message fresh.
                    self.pending_clarifications.remove(&key);
                }
                crate::prime_clarify_memory::ClarifyResolution::Continue { combined } => {
                    effective_message = combined;
                    continued = true;
                    // The raw-answer intent proposal is meaningless for the combined
                    // message; the combined text is reclassified deterministically.
                    intent_proposal = None;
                }
            }
        }
        // The slot bundle is valid ONLY for the message it was computed on. The server
        // computes *continuation* slots on the COMBINED message (when it detects a
        // pending continuation) and *fresh* slots on the raw message; it marks which via
        // `slots.continuation`. Keep the bundle only when that matches the turn we
        // actually produced — so a proposal computed for the wrong message can never
        // shape an action (a continuation never applies raw-answer slots, and a fresh
        // turn never applies combined-message slots).
        if continued != slots.continuation {
            slots = BrainSlotProposals::default();
        }
        let message: &str = &effective_message;

        let summary = self.inspect_state();
        // The live adapter roster, so a brain-proposed agent adapter is honored only
        // when it names one that actually exists (fail closed).
        let adapter_ids: Vec<String> = self
            .adapter_runtime_status()
            .into_iter()
            .map(|a| a.plugin_id)
            .collect();
        let deterministic = classify_intent(message);
        let (intent, intent_source) = match intent_proposal {
            Some(proposal) => {
                crate::prime_intent::reconcile_intent(deterministic, proposal, message)
            }
            None => (deterministic, crate::prime_intent::IntentSource::Deterministic),
        };
        let plan = decide(message, &intent, &summary);

        // Brain-assisted assignment resolution: when the intent is `AssignTask` but the
        // deterministic plan did NOT produce the assignment (the extractors missed the
        // task id or the assignee, so it clarified), a VALIDATED brain proposal of
        // {task_id, agent_id} — both existence-checked against the live state — promotes
        // it to the SAME safe `AssignTask` action the deterministic path would have
        // produced. This is allowed only because assignment is a safe, in-scope action
        // and both ids must already exist (the brain authors no risky action and can name
        // nothing that is not real). On no/low-confidence/unvalidated proposal the
        // deterministic clarify stands (the fallback always exists).
        let mut assign_provenance: Option<relux_core::PrimeAssignSlots> = None;
        let plan = if intent == relux_core::PrimeIntent::AssignTask
            && !matches!(
                &plan,
                PrimePlan::Act {
                    action: PrimeAction::AssignTask { .. },
                    ..
                }
            ) {
            match slots.assign.and_then(|proposal| {
                crate::prime_assign_slots::reconcile_assign_slots(
                    crate::prime::extract_task_id(message).as_deref(),
                    crate::prime::extract_assignee_phrase(message).as_deref(),
                    proposal,
                    &summary,
                )
            }) {
                Some(resolved) => {
                    assign_provenance = Some(relux_core::PrimeAssignSlots {
                        task_id: resolved.task_id.clone(),
                        agent_id: resolved.agent_id.clone(),
                        source: None,
                    });
                    PrimePlan::Act {
                        action: PrimeAction::AssignTask {
                            task_id: resolved.task_id.clone(),
                            agent_id: resolved.agent_id.clone(),
                        },
                        text: format!(
                            "Assigning task {} to agent {}.",
                            resolved.task_id, resolved.agent_id
                        ),
                    }
                }
                None => plan,
            }
        } else {
            plan
        };

        // Brain-assisted by-id update resolution: when the intent is `TaskUpdate` but the
        // deterministic rail could only CLARIFY (it could not find the task and/or field),
        // a VALIDATED brain proposal — task existence-checked, fields sanitized/clamped,
        // status allowlisted, assignee resolved to an existing agent — promotes the turn
        // to the SAME safe `UpdateTask` action. Gated on a `Clarify` so an explicit-but-
        // wrong id/agent (an honest `Reply`) or a refused status is never silently
        // "corrected"; on no/low-confidence/unvalidated proposal the deterministic clarify
        // stands (the fallback always exists). The kernel still validates everything again
        // and enforces the terminal-state guard at apply time.
        let mut update_brain_assisted = false;
        let plan = if intent == relux_core::PrimeIntent::TaskUpdate
            && matches!(&plan, PrimePlan::Clarify { .. })
        {
            match slots.update.and_then(|proposal| {
                crate::prime_update_slots::reconcile_update_slots(
                    crate::prime::extract_task_id(message).as_deref(),
                    proposal,
                    &summary,
                )
            }) {
                Some(resolved) => {
                    update_brain_assisted = true;
                    PrimePlan::Act {
                        action: PrimeAction::UpdateTask {
                            task_id: resolved.task_id.clone(),
                            patch: resolved.patch.to_patch_string(),
                        },
                        text: format!("Updating {}.", resolved.task_id),
                    }
                }
                None => plan,
            }
        } else {
            plan
        };

        // Brain-assisted run-start resolution (the `task.start` write tool): when the intent is
        // `RunStart` but the deterministic plan did NOT produce a `StartRun` Act (the message named
        // no ready task id), a validated brain run slot — the task existence- AND readiness-checked
        // against the live ready queue — promotes the turn to the SAME safe `StartRun` action. The
        // id is taken verbatim from `summary.queued`, so a run can start only for a task that
        // genuinely exists and is ready; on no/unvalidated proposal the deterministic outcome (a
        // clarify or an honest "not ready"/"does not exist" reply) stands. Durable state still flows
        // through `decide` → `prime_execute`.
        let plan = if intent == relux_core::PrimeIntent::RunStart
            && !matches!(
                &plan,
                PrimePlan::Act {
                    action: PrimeAction::StartRun { .. },
                    ..
                }
            ) {
            match slots
                .run
                .and_then(|proposal| crate::prime_write_tools::reconcile_run_start(proposal, &summary))
            {
                Some(task_id) => PrimePlan::Act {
                    action: PrimeAction::StartRun {
                        task_id: task_id.clone(),
                    },
                    text: format!("Starting {task_id}."),
                },
                None => plan,
            }
        } else {
            plan
        };

        // Brain-assisted orchestration goal (the `orchestration.create` write tool): when the
        // intent is `Orchestration`, a VALIDATED brain goal slot REPLACES the keyword-sliced goal
        // that flows into the EXISTING `OrchestrateGoal` action. This both sharpens a goal the
        // deterministic path already accepted and PROMOTES a single-clause clarify whose distinct
        // steps the brain named — but ONLY through the deterministic planner: `reconcile_
        // orchestration_slots` runs `plan_orchestration` and returns `None` unless the composed
        // goal genuinely splits multi-agent, so the planner still owns the role classification,
        // the agent grounding (an agent is matched only against the live roster), the step cap,
        // and the DAG. `prime_execute` re-checks `is_multi_agent` at apply time, so the brain can
        // never fan out a goal the planner would not.
        //
        // CASUAL-CHAT SAFETY: the promotion is gated on `!is_chat_guarded`, the SAME boundary
        // `reconcile_intent` uses to veto a sensitive intent on guarded chat. This is load-bearing
        // here (unlike the assign/update/run-start promotions, where the gate's veto suffices)
        // because the deterministic classifier itself reads a guarded coordination question
        // ("should we split this across a few agents?") as `Orchestration`, so `reconcile_intent`'s
        // veto is a no-op (deterministic == proposal). Without this guard the brain's goal would
        // mint an orchestration from a question. With it, a guarded turn keeps the deterministic
        // clarify and creates nothing — only an EXPLICIT orchestrate/build/do-it request promotes.
        let plan = if intent == relux_core::PrimeIntent::Orchestration
            && !crate::prime::is_chat_guarded(message)
        {
            match slots.orchestration.and_then(|proposal| {
                crate::prime_orchestration_slots::reconcile_orchestration_slots(proposal, &summary)
            }) {
                Some(resolved) => PrimePlan::Act {
                    action: PrimeAction::OrchestrateGoal {
                        goal: resolved.goal.clone(),
                    },
                    text: format!(
                        "Planning an orchestration for \"{}\": {} across {}.",
                        resolved.goal,
                        crate::prime::count_phrase(resolved.plan.steps.len(), "brief"),
                        crate::prime::count_phrase(resolved.plan.agent_labels().len(), "agent"),
                    ),
                },
                None => plan,
            }
        } else {
            plan
        };

        // Brain-assisted orchestration RUN (the `orchestration.start` write tool): when the
        // intent is `OrchestrationRun` but the deterministic plan named no id (it clarified),
        // a validated brain slot — the orchestration existence- AND runnability-checked against
        // the live records (`runnable_orchestration_id`) — promotes the turn to the SAME safe
        // `RunOrchestration` action. The id is taken verbatim from the live records, so a run can
        // start only for an orchestration that genuinely exists and has pending briefs; on
        // no/unvalidated proposal the deterministic clarify stands. Durable state still flows
        // through `decide` → `prime_execute`, which runs the EXISTING governed batch.
        let plan = if intent == relux_core::PrimeIntent::OrchestrationRun
            && !matches!(
                &plan,
                PrimePlan::Act {
                    action: PrimeAction::RunOrchestration { .. },
                    ..
                }
            ) {
            match slots
                .run_orchestration
                .and_then(|proposal| self.runnable_orchestration_id(&proposal.orchestration_id))
            {
                Some(oid) => PrimePlan::Act {
                    action: PrimeAction::RunOrchestration {
                        orchestration_id: oid.clone(),
                    },
                    text: format!("Running orchestration {oid}."),
                },
                None => plan,
            }
        } else {
            plan
        };

        self.record_audit(
            "agent",
            ctx.agent.as_str(),
            "prime:turn",
            Some("message"),
            None,
            Some(&ctx.namespace),
            AuditResult::Success,
            serde_json::json!({
                "intent": format!("{:?}", intent),
                "intent_source": intent_source.as_str(),
                "brain_intent": intent_proposal.map(|p| format!("{:?}", p.intent)),
                "brain_confidence": intent_proposal.map(|p| p.confidence),
                "clarify_continued": continued,
            }),
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
                proposal: None,
                slots: None,
                agent_slots: None,
                admin_slots: None,
                assign_slots: None,
                update: None,
                context_reads: vec![],
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
                proposal: None,
                slots: None,
                agent_slots: None,
                admin_slots: None,
                assign_slots: None,
                update: None,
                context_reads: vec![],
            },
            PrimePlan::Act { action, text } => {
                // Brain-assisted slot sharpening (validated): a create action takes
                // task slots; an `AgentCreation` takes agent slots. Each reconciles
                // against the deterministic value + the live rosters behind the
                // fail-closed gate, and only sharpens an action the deterministic path
                // already decided. `None` (no proposal, low confidence, unknown
                // assignee/adapter, duplicate id) keeps the deterministic slots.
                let resolved_task = match (&action, slots.task) {
                    (
                        PrimeAction::CreateTask { title } | PrimeAction::CreateAndRunTask { title },
                        Some(proposal),
                    ) => crate::prime_slots::reconcile_task_slots(title, proposal, &summary),
                    _ => None,
                };
                let resolved_agent = match (&action, slots.agent) {
                    (PrimeAction::CreateAgent { name, .. }, Some(proposal)) => {
                        crate::prime_agent_slots::reconcile_agent_slots(
                            name,
                            proposal,
                            &summary.all_agent_ids,
                            &adapter_ids,
                        )
                    }
                    _ => None,
                };
                self.prime_execute(
                    ctx,
                    intent,
                    action,
                    text,
                    resolved_task.as_ref(),
                    resolved_agent.as_ref(),
                )?
            }
            PrimePlan::Propose {
                action,
                reason,
                risk,
                text,
            } => {
                // Brain-assisted admin-subject sharpening (advisory). A plugin install
                // or permission grant STAYS gated behind the human approval below; the
                // brain only sharpens the subject the human reviews (a normalized
                // plugin id, or an existing-agent permission subject — validated). On
                // no/low-confidence/unvalidated proposal the deterministic subject
                // stands and no provenance is attached.
                let (action, text, admin_slots) =
                    sharpen_admin_action(action, text, &slots, &summary);
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
                    proposal: None,
                    slots: None,
                    agent_slots: None,
                    admin_slots,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
                }
            }
        };
        // Attach the brain-assisted assignment provenance when the promotion above
        // produced the `AssignTask` action (a deterministic assignment carries none).
        turn.assign_slots = assign_provenance;
        // Mark the by-id update card as brain-resolved when the promotion above produced
        // the `UpdateTask` action. `prime_execute` always builds `turn.update` with
        // `source: None` (the change card); the server later replaces a present-and-marked
        // source with the real model/CLI label. A deterministically-parsed update is left
        // unmarked, so its card shows no brain chip.
        if update_brain_assisted {
            if let Some(u) = turn.update.as_mut() {
                u.source = Some("brain".to_string());
            }
        }
        // Multi-turn clarification memory: record a NEW pending clarification when this
        // turn asked an actionable, resolvable clarifying question, or clear any existing
        // one when the turn resolved/changed it. The combined message is stored as the new
        // original so a further follow-up keeps accumulating context.
        self.update_pending_clarification(&key, &turn, message, now_secs);

        // One central place to offer the next-step buttons the chat surface
        // renders (`docs/RELUX_MASTER_PLAN.md` §11.1 "Prime suggested next
        // actions"). Each is just a pre-written user message, so it can do
        // nothing the user could not type.
        attach_suggestions(&mut turn, message, &summary);
        Ok((turn, intent_source))
    }

    /// The conversation key a pending clarification is stored under: the namespace and
    /// the human actor talking, so two different operators (or namespaces) never share a
    /// pending question. Stable and deterministic.
    fn conversation_key(ctx: &PrimeContext) -> String {
        format!("{}::{}", ctx.namespace.as_str(), ctx.actor)
    }

    /// The current, NON-expired pending clarification for a conversation, if any. Used by
    /// the server to surface the small "waiting for: …" chip; an expired record returns
    /// `None` (and is left to be cleared on the next turn).
    pub fn pending_clarification_for(
        &self,
        ctx: &PrimeContext,
    ) -> Option<relux_core::PendingClarification> {
        let key = Self::conversation_key(ctx);
        let now = self.clock.secs();
        self.pending_clarifications
            .get(&key)
            .filter(|p| now < p.expires_at_secs)
            .cloned()
    }

    /// Read-only preview of whether `message` would CONTINUE a pending clarification, and
    /// if so the combined message + the pending intent. The server calls this BEFORE the
    /// turn (outside the lock would be wrong; it is a quick read under a short lock) so it
    /// can dispatch the slot brain on the COMBINED message + the recorded intent — exactly
    /// the message the kernel will reclassify. Returns `None` when there is no pending
    /// record, it is expired, or the follow-up is a fresh request / cancellation (the same
    /// decision the kernel redoes authoritatively under the lock).
    pub fn continuation_preview(
        &self,
        ctx: &PrimeContext,
        message: &str,
    ) -> Option<(String, relux_core::PrimeIntent)> {
        let key = Self::conversation_key(ctx);
        let pending = self.pending_clarifications.get(&key)?;
        match crate::prime_clarify_memory::resolve_pending(pending, message, self.clock.secs()) {
            crate::prime_clarify_memory::ClarifyResolution::Continue { combined } => {
                Some((combined, pending.intent.clone()))
            }
            _ => None,
        }
    }

    /// Build the natural reply turn when the user explicitly cancels a pending
    /// clarification ("never mind"). Action-free: nothing is created or run, and the
    /// intent is `DirectAnswer` so the validated wording path leaves it alone.
    fn clarification_cancelled_turn(
        &mut self,
        ctx: &PrimeContext,
        pending: &relux_core::PendingClarification,
    ) -> PrimeTurn {
        self.record_audit(
            "agent",
            ctx.agent.as_str(),
            "prime:clarify_cancelled",
            Some("message"),
            None,
            Some(&ctx.namespace),
            AuditResult::Success,
            serde_json::json!({ "intent": format!("{:?}", pending.intent) }),
        );
        PrimeTurn {
            intent: relux_core::PrimeIntent::DirectAnswer,
            reply: "Okay, I've set that aside. What would you like to do instead?".to_string(),
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
            proposal: None,
            slots: None,
            agent_slots: None,
            admin_slots: None,
            assign_slots: None,
            update: None,
            context_reads: vec![],
        }
    }

    /// Record or clear the pending clarification for a conversation after a turn.
    ///
    /// Records a NEW record only when the turn is an actionable, *resolvable* `Clarify`
    /// (assignment / task creation — the intents whose clarify a follow-up can actually
    /// turn into an action); any other turn (a resolved action, a plain reply, an
    /// unresolvable clarify) clears any existing record. The map is bounded: at most
    /// [`MAX_PENDING_CLARIFICATIONS`] entries, evicting the oldest by creation time.
    fn update_pending_clarification(
        &mut self,
        key: &str,
        turn: &PrimeTurn,
        effective_message: &str,
        now_secs: u64,
    ) {
        let resolvable = turn.disposition == PrimeDisposition::NeedsClarification
            && crate::prime_clarify_memory::is_resolvable_clarify_intent(&turn.intent);
        if !resolvable {
            self.pending_clarifications.remove(key);
            return;
        }
        let pending = relux_core::PendingClarification {
            original_message: crate::prime_clarify_memory::clamp(
                effective_message,
                crate::prime_clarify_memory::MAX_ORIGINAL_CHARS,
            ),
            intent: turn.intent.clone(),
            needs: crate::prime::clarify_needs_label(&turn.intent, effective_message),
            question: crate::prime_clarify_memory::clamp(
                &turn.reply,
                crate::prime_clarify_memory::MAX_QUESTION_CHARS,
            ),
            created_at_secs: now_secs,
            expires_at_secs: now_secs.saturating_add(crate::prime_clarify_memory::CLARIFY_TTL_SECS),
            source: "deterministic".to_string(),
        };
        // Bound the map: if inserting a new key would exceed the cap, evict the oldest
        // record (smallest creation time) so memory stays small and deterministic.
        if !self.pending_clarifications.contains_key(key)
            && self.pending_clarifications.len() >= MAX_PENDING_CLARIFICATIONS
        {
            if let Some(oldest) = self
                .pending_clarifications
                .iter()
                .min_by(|a, b| {
                    a.1.created_at_secs
                        .cmp(&b.1.created_at_secs)
                        .then_with(|| a.0.cmp(b.0))
                })
                .map(|(k, _)| k.clone())
            {
                self.pending_clarifications.remove(&oldest);
            }
        }
        self.pending_clarifications.insert(key.to_string(), pending);
    }

    /// Record one completed Prime turn into the conversation's bounded history, so the NEXT
    /// turn's brain can interpret a follow-up in context (`docs/prime-processing-audit.md`
    /// "Bounded conversation memory"; see [`crate::prime_history`]).
    ///
    /// `user_message` is the message the turn answered (the COMBINED message on a clarification
    /// continuation), `turn` is the finished turn (its `reply` is the FINAL user-visible reply —
    /// the server records AFTER shaping, so a brain-shaped / after-action wording is what is
    /// stored, never an earlier draft), and `context_reads` are the read-only context reads
    /// consulted ([`relux_core::PrimeContextRead`]) — each stored as its name plus its bounded
    /// one-line summary, never the tool's result body.
    /// The record is built secret-redacted + clamped by [`crate::prime_history::build_turn`], the
    /// per-conversation list is bounded to [`crate::prime_history::MAX_HISTORY_TURNS`] (oldest
    /// evicted), and the number of conversations is bounded to
    /// [`crate::prime_history::MAX_HISTORY_CONVERSATIONS`] (the conversation whose most-recent turn
    /// is oldest is evicted when a NEW conversation would exceed the cap). This stores advisory
    /// context only; it grants no authority and is never consulted by an action/gate path.
    pub fn record_conversation_turn(
        &mut self,
        ctx: &PrimeContext,
        user_message: &str,
        turn: &PrimeTurn,
        context_reads: &[relux_core::PrimeContextRead],
    ) {
        let key = Self::conversation_key(ctx);
        let now_secs = self.clock.secs();
        let record = crate::prime_history::build_turn(user_message, turn, context_reads, now_secs);
        // Bound the number of distinct conversations: when recording into a NEW conversation would
        // exceed the cap, evict the conversation whose most-recent turn is the oldest (the least
        // recently active), so the live conversations are the ones kept.
        if !self.conversation_histories.contains_key(&key)
            && self.conversation_histories.len() >= crate::prime_history::MAX_HISTORY_CONVERSATIONS
        {
            if let Some(stalest) = self
                .conversation_histories
                .iter()
                .min_by(|a, b| {
                    let a_last = a.1.last().map(|t| t.created_at_secs).unwrap_or(0);
                    let b_last = b.1.last().map(|t| t.created_at_secs).unwrap_or(0);
                    a_last.cmp(&b_last).then_with(|| a.0.cmp(b.0))
                })
                .map(|(k, _)| k.clone())
            {
                self.conversation_histories.remove(&stalest);
                // Drop the evicted conversation's rolling summary too, so the two maps stay in
                // sync and the summary memory is bounded by the same conversation cap.
                self.conversation_summaries.remove(&stalest);
            }
        }
        let history = self.conversation_histories.entry(key.clone()).or_default();
        // Push the new turn, keeping the ring bounded; any turn aged OUT of the front is folded
        // into the conversation's rolling, deterministic, bounded summary so a long thread keeps a
        // compact memory of older turns instead of dropping them on the floor. Folding is pure +
        // off-network (it runs under the kernel lock).
        let evicted = crate::prime_history::push_bounded(history, record);
        if !evicted.is_empty() {
            let summary = self.conversation_summaries.entry(key).or_default();
            for turn in &evicted {
                crate::prime_history::fold_evicted_turn(summary, turn, now_secs);
            }
        }
    }

    /// Render the conversation's recent history into a bounded, clearly-labelled BACKGROUND
    /// context block for the brain's prompt, or `""` when there is none. Read-only; the server
    /// calls this in the pre-turn preflight lock and threads the string into the decision prompt
    /// (see [`crate::prime_history::render_context`]). Advisory context only — never an instruction.
    pub fn recent_conversation_context(&self, ctx: &PrimeContext) -> String {
        let key = Self::conversation_key(ctx);
        let summary = self.conversation_summaries.get(&key);
        let history = self.conversation_histories.get(&key);
        match (summary, history) {
            // The common path: render the compacted summary of older turns (when any) at the top
            // of the same BACKGROUND block, followed by the verbatim recent ring. Both empty ->
            // "" (the empty-history prompt identity is preserved exactly).
            (None, None) => String::new(),
            (s, h) => crate::prime_history::render_context_with_summary(
                s,
                h.map(Vec::as_slice).unwrap_or(&[]),
            ),
        }
    }

    /// Clear a conversation's memory — its bounded turn history, its rolling compacted summary of
    /// older turns, AND any pending clarification — for the "clear conversation" / reset action.
    /// Returns `true` when anything was actually cleared. Drops only advisory memory; no durable
    /// entity (task/run/agent) is touched, so a reset can never lose real work.
    pub fn clear_conversation(&mut self, ctx: &PrimeContext) -> bool {
        let key = Self::conversation_key(ctx);
        let had_history = self.conversation_histories.remove(&key).is_some();
        let had_summary = self.conversation_summaries.remove(&key).is_some();
        let had_pending = self.pending_clarifications.remove(&key).is_some();
        had_history || had_summary || had_pending
    }

    /// Apply a brain-suggested, already-clamped priority to a freshly created task.
    /// A no-op when the brain offered no priority. The value was clamped to the
    /// supported range by [`crate::prime_slots::parse_task_slots`], so this only
    /// sets it; nothing else about the task changes.
    fn apply_slot_priority(
        &mut self,
        task: &TaskId,
        slots: Option<&crate::prime_slots::ResolvedTaskSlots>,
    ) {
        let Some(priority) = slots.and_then(|s| s.priority) else {
            return;
        };
        let now = self.clock.tick();
        if let Some(t) = self.tasks.get_mut(task) {
            t.priority = priority;
            t.updated_at = now;
        }
    }

    /// Build the honest, action-free turn returned when a by-id `UpdateTask` cannot be
    /// applied (the task vanished, is already terminal, or there was nothing safe to
    /// change). It changes no durable state and asks the operator to refine — never a
    /// fake "updated" claim.
    fn prime_update_refused(
        &self,
        intent: relux_core::PrimeIntent,
        action: PrimeAction,
        reply: String,
    ) -> PrimeTurn {
        PrimeTurn {
            intent,
            reply,
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
            proposal: None,
            slots: None,
            agent_slots: None,
            admin_slots: None,
            assign_slots: None,
            update: None,
            context_reads: vec![],
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
        slots: Option<&crate::prime_slots::ResolvedTaskSlots>,
        agent_slots: Option<&crate::prime_agent_slots::ResolvedAgentSlots>,
    ) -> Result<PrimeTurn, KernelError> {
        match &action {
            PrimeAction::CreateTask { title } => {
                // The brain may sharpen the slots (validated); otherwise the
                // deterministic title stands. Details fold into the task input; an
                // assignee is honored only when it named an EXISTING agent
                // (`prime_slots::reconcile_task_slots` already validated it).
                let eff_title = slots.map(|s| s.title.clone()).unwrap_or_else(|| title.clone());
                let mut input = serde_json::json!({ "prime_request": eff_title });
                if let Some(details) = slots.and_then(|s| s.details.as_deref()) {
                    input["details"] = serde_json::Value::String(details.to_string());
                }
                let task = self.create_task(
                    &eff_title,
                    input,
                    &ctx.actor,
                    &ctx.namespace,
                    vec![],
                );
                // Assign to the brain-suggested existing agent when present,
                // otherwise to Prime so the work is immediately runnable when the
                // user says "start it" (assigning to self is within Prime's scope).
                let assignee = slots.and_then(|s| s.assignee.clone());
                let assigned = match &assignee {
                    Some(id) => AgentId::new(id.clone()),
                    None => ctx.agent.clone(),
                };
                self.assign_task(&task, &assigned)?;
                self.apply_slot_priority(&task, slots);
                let head = if slots.is_some() {
                    format!("Creating a task: \"{eff_title}\".")
                } else {
                    text
                };
                let reply = format!(
                    "{head} Created {task} and assigned it to {assigned}. It is ready to run whenever you are."
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
                    proposal: None,
                    slots: slots.map(|s| relux_core::PrimeTaskSlots {
                        title: eff_title.clone(),
                        details: s.details.clone(),
                        assignee,
                        priority: s.priority,
                        source: None,
                    }),
                    agent_slots: None,
                    admin_slots: None,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
                })
            }
            PrimeAction::CreateAndRunTask { title } => {
                // Title/details/priority may be brain-sharpened, but the assignee is
                // NOT overridden here: this path immediately starts a run, and the
                // task carries a required permission only Prime is wired to satisfy,
                // so the run stays assigned to Prime (the brain can never reassign a
                // task it would auto-run onto an agent that may lack the grant).
                let eff_title = slots.map(|s| s.title.clone()).unwrap_or_else(|| title.clone());
                let mut input = serde_json::json!({ "prime_request": eff_title });
                if let Some(details) = slots.and_then(|s| s.details.as_deref()) {
                    input["details"] = serde_json::Value::String(details.to_string());
                }
                let task = self.create_task(
                    &eff_title,
                    input,
                    &ctx.actor,
                    &ctx.namespace,
                    vec![Permission::new("tool:relux-tools-echo:say").unwrap()],
                );
                self.assign_task(&task, &ctx.agent)?;
                self.apply_slot_priority(&task, slots);
                let run = self.start_run(&task)?;

                let head = if slots.is_some() {
                    format!("Creating and running task: \"{eff_title}\".")
                } else {
                    text
                };
                let reply = format!(
                    "{head} Created {task} and started {run}. The task is now running and awaiting further action from the assigned agent."
                );
                // Provenance reflects only what was applied: the assignee is never
                // applied on this path, and a proposal that contributed nothing but
                // a (dropped) assignee shows no chip.
                let provenance = slots.and_then(|s| {
                    let changed = eff_title.trim() != title.trim()
                        || s.details.is_some()
                        || s.priority.is_some();
                    changed.then(|| relux_core::PrimeTaskSlots {
                        title: eff_title.clone(),
                        details: s.details.clone(),
                        assignee: None,
                        priority: s.priority,
                        source: None,
                    })
                });
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
                    proposal: None,
                    slots: provenance,
                    agent_slots: None,
                    admin_slots: None,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
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
                    proposal: None,
                    slots: None,
                    agent_slots: None,
                    admin_slots: None,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
                })
            }
            PrimeAction::CreateAgent {
                name,
                adapter_plugin,
            } => {
                // The brain may sharpen the agent slots (validated against the live
                // agent + adapter rosters); otherwise the deterministic name stands.
                // The id is the normalized handle (brain id when present, else derived
                // the same way the deterministic path always did). The description and
                // adapter are taken from the brain only when validated.
                let eff_name = agent_slots.map(|s| s.name.clone()).unwrap_or_else(|| name.clone());
                let agent_id_str = agent_slots
                    .map(|s| s.id.clone())
                    .unwrap_or_else(|| name.to_lowercase().replace(' ', "-"));
                let description = agent_slots
                    .and_then(|s| s.description.clone())
                    .unwrap_or_else(|| "Agent created by Prime".to_string());
                let adapter_id = agent_slots
                    .and_then(|s| s.adapter.clone())
                    .unwrap_or_else(|| adapter_plugin.clone());
                let adapter = PluginId::new(adapter_id.clone());
                // A validated starter persona (brain-proposed, bounded, control-char
                // stripped) gives the operative a usable voice; the deterministic path
                // still creates it with no persona.
                let persona = agent_slots.and_then(|s| s.persona.clone());
                let agent_id = self.create_agent(
                    &agent_id_str,
                    &eff_name,
                    &description,
                    &adapter,
                    &ctx.namespace,
                    persona.clone(),
                    vec![], // No special permissions by default
                )?;
                let head = if agent_slots.is_some() {
                    format!("Creating agent \"{eff_name}\".")
                } else {
                    text
                };
                let reply = format!("{head} Created agent {agent_id}.");
                // Provenance reflects only what the brain genuinely contributed and the
                // kernel applied: a no-op echo never reaches here (reconcile returned
                // None), so a present `agent_slots` always sharpened the create.
                let provenance = agent_slots.map(|s| relux_core::PrimeAgentSlots {
                    name: eff_name.clone(),
                    id: agent_id_str.clone(),
                    description: s.description.clone(),
                    adapter: s.adapter.clone(),
                    notes: s.notes.clone(),
                    persona: persona.clone(),
                    source: None,
                });
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
                    proposal: None,
                    slots: None,
                    agent_slots: provenance,
                    admin_slots: None,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
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
                    proposal: None,
                    slots: None,
                    agent_slots: None,
                    admin_slots: None,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
                })
            }
            PrimeAction::UpdateTask { task_id, patch } => {
                // A by-id update is a SAFE mutating action: edit an existing, non-terminal
                // task in place. Every field in `patch` was validated before it reached
                // here (decide's rail or the brain-promotion chokepoint); this re-checks
                // existence, enforces the terminal-state guard, and applies only the
                // allowlisted fields — so even a stale/forged patch can never edit a
                // finished task or set a machine-driven status.
                let task_id = task_id.clone();
                let tid = TaskId::new(task_id.clone());
                let patch = crate::prime_update_slots::TaskUpdatePatch::from_patch_str(patch)
                    .unwrap_or_default();

                let Some((status, namespace)) = self
                    .tasks
                    .get(&tid)
                    .map(|t| (t.status.clone(), t.namespace_id.clone()))
                else {
                    return Ok(self.prime_update_refused(
                        intent,
                        action,
                        format!("Task with ID '{task_id}' does not exist. Please provide a valid task ID."),
                    ));
                };
                if crate::prime_update_slots::is_terminal_status(&status) {
                    return Ok(self.prime_update_refused(
                        intent,
                        action,
                        format!(
                            "Task {task_id} is already {} — I won't change a finished task.",
                            crate::prime_update_slots::status_label(&status)
                        ),
                    ));
                }
                if patch.is_empty() {
                    return Ok(self.prime_update_refused(
                        intent,
                        action,
                        "There was nothing to change on that task.".to_string(),
                    ));
                }

                let now = self.clock.tick();
                let mut applied: Vec<relux_core::PrimeTaskChange> = Vec::new();

                // Reassignment goes through `assign_task` (validates the agent exists and
                // moves the task to Queued). Applied first so a co-requested status wins.
                if let Some(agent) = &patch.assignee {
                    self.assign_task(&tid, &AgentId::new(agent.clone()))?;
                    applied.push(relux_core::PrimeTaskChange {
                        field: "assignee".to_string(),
                        value: agent.clone(),
                    });
                }
                if let Some(t) = self.tasks.get_mut(&tid) {
                    if let Some(title) = &patch.title {
                        t.title = title.clone();
                        applied.push(relux_core::PrimeTaskChange {
                            field: "title".to_string(),
                            value: title.clone(),
                        });
                    }
                    if let Some(details) = &patch.details {
                        t.input["details"] = serde_json::Value::String(details.clone());
                        applied.push(relux_core::PrimeTaskChange {
                            field: "details".to_string(),
                            value: details.clone(),
                        });
                    }
                    if let Some(priority) = patch.priority {
                        t.priority = priority;
                        applied.push(relux_core::PrimeTaskChange {
                            field: "priority".to_string(),
                            value: priority.to_string(),
                        });
                    }
                    if let Some(st) = &patch.status {
                        // Defense in depth: only the operator-settable allowlist is honored.
                        if crate::prime_update_slots::is_settable_status(st) {
                            t.status = st.clone();
                            applied.push(relux_core::PrimeTaskChange {
                                field: "status".to_string(),
                                value: crate::prime_update_slots::status_label(st).to_string(),
                            });
                        }
                    }
                    t.updated_at = now;
                }

                if applied.is_empty() {
                    return Ok(self.prime_update_refused(
                        intent,
                        action,
                        "There was nothing I could safely change on that task.".to_string(),
                    ));
                }

                let change_pairs: std::collections::BTreeMap<String, String> = applied
                    .iter()
                    .map(|c| (c.field.clone(), c.value.clone()))
                    .collect();
                self.record_audit(
                    "agent",
                    ctx.agent.as_str(),
                    "task:update",
                    Some("task"),
                    Some(task_id.as_str()),
                    Some(&namespace),
                    AuditResult::Success,
                    serde_json::json!({ "changes": change_pairs }),
                );

                let summary_line = applied
                    .iter()
                    .map(|c| format!("{} → {}", c.field, c.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                let reply = format!("{text} Updated {task_id}: {summary_line}.");
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
                    proposal: None,
                    slots: None,
                    agent_slots: None,
                    admin_slots: None,
                    assign_slots: None,
                    update: Some(relux_core::PrimeTaskUpdate {
                        task_id,
                        changes: applied,
                        source: None,
                    }),
                    context_reads: vec![],
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
                    proposal: None,
                    slots: None,
                    agent_slots: None,
                    admin_slots: None,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
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
                        proposal: None,
                        slots: None,
                        agent_slots: None,
                        admin_slots: None,
                        assign_slots: None,
                        update: None,
                        context_reads: vec![],
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
                    proposal: None,
                    slots: None,
                    agent_slots: None,
                    admin_slots: None,
                    assign_slots: None,
                    update: None,
                    context_reads: vec![],
                }),
                Err(e) => Err(e),
            },
            PrimeAction::RunOrchestration { orchestration_id } => {
                // Run (or continue) the EXISTING governed batch for this orchestration — the
                // SAME `run_orchestration` engine the blocking `/run` API and the CLI drive
                // (`docs/RELUX_MASTER_PLAN.md` §10.4). The id is validated against the live
                // records first: an unknown id is an honest, action-free reply (fail closed,
                // never a faked run), exactly like an unknown task on `StartRun`. Bounded by
                // the same defaults the blocking endpoint uses (max 25, concurrency 2); each
                // brief still gates at run time through its assigned agent's adapter.
                let oid = OrchestrationId::new(orchestration_id.clone());
                if self.orchestration(&oid).is_none() {
                    return Ok(PrimeTurn {
                        intent,
                        reply: format!(
                            "There is no orchestration with id '{orchestration_id}'. Ask me to list the orchestrations to see what exists."
                        ),
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
                        proposal: None,
                        slots: None,
                        agent_slots: None,
                        admin_slots: None,
                        assign_slots: None,
                        update: None,
                        context_reads: vec![],
                    });
                }
                match self.run_orchestration(&oid, 25, 2) {
                    Ok(result) => {
                        let reply = format!("{text} {} {}", result.summary, result.next_action);
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
                            proposal: None,
                            slots: None,
                            agent_slots: None,
                            admin_slots: None,
                            assign_slots: None,
                            update: None,
                            context_reads: vec![],
                        })
                    }
                    Err(e) => Err(e),
                }
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
                created_agent: None,
                approval: None,
                invoked_tool: None,
                tool_output: None,
                tool_error: None,
                suggested_actions: Vec::new(),
                proposal: None,
                slots: None,
                agent_slots: None,
                admin_slots: None,
                assign_slots: None,
                update: None,
                context_reads: vec![],
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
                proposal: None,
                slots: None,
                agent_slots: None,
                admin_slots: None,
                assign_slots: None,
                update: None,
                context_reads: vec![],
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
            Some(ToolExecutability::NeedsApproval) => {
                let note = format!(
                    "{label} is configured as a higher-risk tool that requires approval, so I will not run it directly; lower its risk (or mark it low-risk + auto-approve) to make it directly callable"
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
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string(), RunFailureClass::AdapterMissing);
                return Err(err);
            }
            None => {
                let err = KernelError::AdapterRuntimeNotConfigured {
                    plugin: adapter.to_string(),
                };
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string(), RunFailureClass::AdapterMissing);
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
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string(), RunFailureClass::AdapterMissing);
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
                self.fail_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &err.to_string(), RunFailureClass::AdapterMissing);
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
        // If this run is a resume (its `resumed_from` lineage is set) and carries a
        // resumable provider session, thread `--resume <session_id>` so the adapter
        // continues that session instead of starting fresh (the session id is
        // already sanitized + argv-safe). A normal run has no `resumed_from`, so it
        // always gets the fresh args — resume never leaks onto a cold run.
        let resume_session_id = self
            .runs
            .get(&run_id)
            .filter(|r| r.resumed_from.is_some())
            .and_then(|r| r.session.as_ref())
            .filter(|s| s.resume_supported)
            .map(|s| s.adapter_session_id.clone());
        let args = match resume_session_id.as_deref() {
            Some(sid) => crate::adapter::build_resume_adapter_args(&config.kind, sid),
            None => crate::adapter::build_adapter_args(&config.kind),
        };
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
                let parsed = relux_core::parse_adapter_result(&outcome.stdout, config_kind.clone());
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
                // Capture the bounded, redacted run-log tail (stdout/stderr split
                // into lines + system framing) for the live logs/tail surface.
                self.capture_cli_run_log(&run_id, &config_kind, &binary, &outcome);

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
                    // An errored run may still have established a provider session
                    // before failing — capture its identity for handoff/audit.
                    self.set_run_session(&run_id, parsed.session_id.as_deref(), config_kind.clone());
                    // Classify from the model's own error text: a transient cause
                    // (rate limit, overload) becomes a retryable TransientProvider;
                    // any other error result is OutputValidation (operator review).
                    let envelope_class = match classify_failure(&parsed.text, false) {
                        RunFailureClass::TransientProvider => RunFailureClass::TransientProvider,
                        RunFailureClass::Timeout => RunFailureClass::Timeout,
                        RunFailureClass::AuthRequired => RunFailureClass::AuthRequired,
                        _ => RunFailureClass::OutputValidation,
                    };
                    self.fail_cli_run(
                        &run_id,
                        task_id,
                        namespace.as_ref(),
                        adapter,
                        &reason,
                        envelope_class,
                    );
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
                // Capture the provider session identity for durable handoff/resume
                // metadata (Claude `--output-format json` emits a `session_id`).
                self.set_run_session(&run_id, parsed.session_id.as_deref(), config_kind);
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
                let reason = if outcome.cancelled {
                    format!("adapter '{}' was cancelled by operator", binary)
                } else if outcome.timed_out {
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
                        "cancelled": outcome.cancelled,
                        "stdout": outcome.stdout,
                        "stderr": outcome.stderr,
                        "duration_ms": outcome.duration_ms,
                    }),
                );
                // Capture the bounded, redacted run-log tail for the logs surface
                // (a failed/cancelled run still has stdout/stderr worth showing).
                self.capture_cli_run_log(&run_id, &config_kind, &binary, &outcome);
                self.set_run_metrics(&run_id, outcome.duration_ms, None, None);
                // A cancel is intentional + terminal: mark the run Cancelled (NOT
                // Failed) with the Cancelled failure class, so it never auto-retries
                // and never reads as an operator-action failure. A wall-clock
                // timeout is a safe-to-retry transient; a non-zero exit is genuinely
                // unclassifiable (the cause could be anything) so it stays Unknown —
                // NOT auto-retried, since a coding-agent run can mutate a workspace.
                if outcome.cancelled {
                    self.cancel_cli_run(&run_id, task_id, namespace.as_ref(), adapter, &reason);
                    return Err(KernelError::AdapterExecutionFailed {
                        plugin: adapter.to_string(),
                        message: reason,
                    });
                }
                let exit_class = if outcome.timed_out {
                    RunFailureClass::Timeout
                } else {
                    classify_failure(&reason, false)
                };
                self.fail_cli_run(
                    &run_id,
                    task_id,
                    namespace.as_ref(),
                    adapter,
                    &reason,
                    exit_class,
                );
                Err(KernelError::AdapterExecutionFailed {
                    plugin: adapter.to_string(),
                    message: reason,
                })
            }
            Err(e) => {
                let reason = format!("failed to spawn adapter '{binary}': {e}");
                // Capture a system-only run-log tail so the logs surface honestly
                // shows the spawn failure (there is no process stdout/stderr).
                self.capture_spawn_error_log(&run_id, &config_kind, &binary, &reason);
                // A spawn failure on a binary that resolved on PATH is an
                // environment/exec fault — classify from the text (Unknown unless
                // the OS error names a transient cause).
                let spawn_class = classify_failure(&reason, false);
                self.fail_cli_run(
                    &run_id,
                    task_id,
                    namespace.as_ref(),
                    adapter,
                    &reason,
                    spawn_class,
                );
                Err(KernelError::AdapterExecutionFailed {
                    plugin: adapter.to_string(),
                    message: reason,
                })
            }
        }
    }

    /// Mark a CLI run + its task failed and audit the failure. Shared by every
    /// honest failure exit of [`execute_cli_run`]. The caller passes the structured
    /// [`RunFailureClass`] it knows definitively (a missing/disabled adapter, a
    /// timeout, an error envelope classified from its text), so the run records an
    /// honest class + retry state (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §7).
    fn fail_cli_run(
        &mut self,
        run_id: &RunId,
        task_id: &TaskId,
        namespace: Option<&NamespaceId>,
        adapter: &PluginId,
        reason: &str,
        class: RunFailureClass,
    ) {
        let agent = self
            .runs
            .get(run_id)
            .map(|r| r.agent_id.as_str().to_string())
            .unwrap_or_else(|| "agent".to_string());
        let _ = self.fail_run_classified(run_id, reason, class);
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

    /// Mark a CLI run **cancelled** (not failed) + its task failed, and audit it.
    /// Used only when an operator killed the adapter mid-flight
    /// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26). The run records the terminal
    /// [`RunStatus::Cancelled`] with the [`RunFailureClass::Cancelled`] class (so the
    /// UI shows a Cancelled chip + remediation and the recovery projections never
    /// treat it as a retry/operator-action failure); the task goes Failed so it is
    /// not stuck Running and the operator can start it fresh.
    fn cancel_cli_run(
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
        let _ = self.cancel_run(run_id, reason);
        let _ = self.fail_task(task_id);
        self.record_audit(
            "agent",
            &agent,
            "adapter:cancel",
            Some("adapter"),
            Some(adapter.as_str()),
            namespace,
            AuditResult::Failed,
            serde_json::json!({ "run": run_id.as_str(), "reason": reason }),
        );
    }

    /// Mark a run **cancelled**: terminal [`RunStatus::Cancelled`] + the
    /// [`RunFailureClass::Cancelled`] class, no retry state (a cancel is never
    /// auto-retried), with a `run_cancelled` transcript event + audit. The honest
    /// counterpart to [`Self::fail_run_classified`] for an intentional operator stop
    /// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26).
    pub fn cancel_run(&mut self, run_id: &RunId, reason: &str) -> Result<(), KernelError> {
        let ended = self.clock.tick();
        let (agent_id, task_id) = {
            let run = self
                .runs
                .get_mut(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            run.status = RunStatus::Cancelled;
            run.ended_at = Some(ended);
            run.error = Some(reason.to_string());
            run.failure_class = Some(RunFailureClass::Cancelled);
            run.retry = None;
            (run.agent_id.clone(), run.task_id.clone())
        };
        let task_namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());
        self.push_run_event(
            run_id,
            "run_cancelled",
            "kernel",
            reason,
            serde_json::json!({ "failure_class": RunFailureClass::Cancelled.as_str() }),
        );
        self.record_audit(
            "agent",
            agent_id.as_str(),
            "run:cancel",
            Some("run"),
            Some(run_id.as_str()),
            task_namespace.as_ref(),
            AuditResult::Failed,
            serde_json::Value::Null,
        );
        Ok(())
    }

    /// Capture a bounded, redacted **run-log tail** from a finished CLI adapter
    /// run (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10). Built from the adapter's
    /// already-redacted, byte-capped stdout/stderr (each split into per-line
    /// entries) framed by kernel-authored `system` lines (spawn + outcome). The
    /// builder re-redacts every line, clamps line length, and clamps the total to
    /// [`relux_core::MAX_LOG_LINES`] (oldest dropped, count recorded), so the
    /// record is always bounded. One log per run id; a resume/retry is a distinct
    /// run id, so this never overwrites a prior run's log.
    ///
    /// Honest note: Relux captures the run's FINAL output (the synchronous spawn
    /// does not stream chunks during the run), so stdout lines are grouped, then
    /// stderr lines — not interleaved by real time. Live, interleaved streaming is
    /// a future seam (there is no `onLog` callback on the spawn yet).
    fn capture_cli_run_log(
        &mut self,
        run_id: &RunId,
        kind: &AdapterKind,
        binary: &str,
        outcome: &crate::adapter::AdapterRunOutcome,
    ) {
        let mut builder = relux_core::RunLogBuilder::new();
        builder.mark_stream_truncation(outcome.stdout_truncated, outcome.stderr_truncated);
        builder.push_system(format!("spawned {} adapter '{}'", kind.as_str(), binary));
        builder.push_output(relux_core::RunLogSource::Stdout, &outcome.stdout);
        builder.push_output(relux_core::RunLogSource::Stderr, &outcome.stderr);
        let outcome_line = if outcome.cancelled {
            format!("adapter cancelled by operator after {} ms", outcome.duration_ms)
        } else if outcome.timed_out {
            format!("adapter timed out after {} ms", outcome.duration_ms)
        } else {
            format!(
                "adapter exited with code {} in {} ms",
                outcome
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                outcome.duration_ms
            )
        };
        builder.push_system(outcome_line);
        self.run_logs.insert(run_id.clone(), builder.build(run_id.clone()));
    }

    /// Capture a system-only run-log tail for a run whose adapter process never
    /// started (a spawn error), so the logs surface honestly shows the failure
    /// rather than blanking. There is no process stdout/stderr in this case.
    fn capture_spawn_error_log(
        &mut self,
        run_id: &RunId,
        kind: &AdapterKind,
        binary: &str,
        message: &str,
    ) {
        let mut builder = relux_core::RunLogBuilder::new();
        builder.push_system(format!("failed to spawn {} adapter '{}'", kind.as_str(), binary));
        builder.push_system(message);
        self.run_logs.insert(run_id.clone(), builder.build(run_id.clone()));
    }

    /// Mark a run failed with an error message and record it on the transcript.
    ///
    /// The failure is classified from its reason text (the deterministic rail);
    /// callers with a more specific structured signal use
    /// [`Self::fail_run_classified`] instead.
    pub fn fail_run(&mut self, run_id: &RunId, error: &str) -> Result<(), KernelError> {
        let class = classify_failure(error, false);
        self.fail_run_classified(run_id, error, class)
    }

    /// Mark a run failed and stamp its structured [`RunFailureClass`] +
    /// bounded-retry state (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §7).
    ///
    /// A failed run records WHY it failed (the class), and — for an auto-retryable
    /// transient class only — a [`RunRetryState`] scheduling the next attempt on the
    /// bounded `[2m,10m,30m,2h]` backoff (the attempt index is the run's
    /// `retried_from` lineage length). A non-retryable class records only the class
    /// (it waits for an operator). The retry is never fired by a background thread;
    /// it becomes eligible at a real instant and is consumed by `retry_run` or the
    /// next autonomy tick.
    pub fn fail_run_classified(
        &mut self,
        run_id: &RunId,
        error: &str,
        class: RunFailureClass,
    ) -> Result<(), KernelError> {
        let ended = self.clock.tick();
        let attempt = self.transient_attempt_for(run_id);
        let retry = RunRetryState::plan(class, attempt, real_now_secs());
        let (agent_id, task_id) = {
            let run = self
                .runs
                .get_mut(run_id)
                .ok_or_else(|| KernelError::UnknownRun(run_id.to_string()))?;
            run.status = RunStatus::Failed;
            run.ended_at = Some(ended);
            run.error = Some(error.to_string());
            run.failure_class = Some(class);
            run.retry = retry;
            (run.agent_id.clone(), run.task_id.clone())
        };
        let task_namespace = self.tasks.get(&task_id).map(|t| t.namespace_id.clone());
        self.push_run_event(
            run_id,
            "run_failed",
            "kernel",
            error,
            serde_json::json!({ "failure_class": class.as_str() }),
        );
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

    /// The transient-retry attempt index for `run_id`: the length of its
    /// `retried_from` lineage (0 for an original run, 1 for the first retry, …).
    /// Bounded against a pathological cycle. This is the 0-based attempt the
    /// bounded backoff schedule is indexed by.
    fn transient_attempt_for(&self, run_id: &RunId) -> u32 {
        let mut attempt: u32 = 0;
        let mut cursor = self.runs.get(run_id).and_then(|r| r.retried_from.clone());
        // The lineage can never exceed the transient budget by more than a small
        // margin; cap the walk well above it as a cycle backstop.
        while let Some(prev) = cursor {
            attempt = attempt.saturating_add(1);
            if attempt > 64 {
                break;
            }
            cursor = self.runs.get(&prev).and_then(|r| r.retried_from.clone());
        }
        attempt
    }

    /// The newest run per task (by monotonic run id), as a deterministic,
    /// id-sorted list. The basis for the run-recovery projections below — a task's
    /// CURRENT disposition is its latest run, not any earlier attempt.
    fn newest_run_per_task(&self) -> Vec<&Run> {
        let mut newest: HashMap<&TaskId, &Run> = HashMap::new();
        for run in self.runs.values() {
            newest
                .entry(&run.task_id)
                .and_modify(|cur| {
                    if run.id.0 > cur.id.0 {
                        *cur = run;
                    }
                })
                .or_insert(run);
        }
        let mut out: Vec<&Run> = newest.into_values().collect();
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        out
    }

    /// The failed runs whose bounded transient retry is ELIGIBLE at `now_secs`:
    /// the task's newest run failed with an auto-retryable class, its retry state
    /// is scheduled (not exhausted) with `not_before_secs <= now_secs`, and the
    /// task is still assigned. Read-only — the honest "retry-ready" projection a
    /// manual retry or an autonomy tick consumes. No background scheduler.
    pub fn transient_retry_ready(&self, now_secs: u64) -> Vec<RunId> {
        self.newest_run_per_task()
            .into_iter()
            .filter(|run| {
                run.status == RunStatus::Failed
                    && run.failure_class.map(|c| c.retryable()).unwrap_or(false)
                    && run.retry.as_ref().map(|r| r.eligible_at(now_secs)).unwrap_or(false)
                    && self
                        .tasks
                        .get(&run.task_id)
                        .map(|t| t.assigned_agent.is_some())
                        .unwrap_or(false)
            })
            .map(|run| run.id.clone())
            .collect()
    }

    /// Count of tasks whose newest run failed with a class that needs an operator
    /// to act (auth/adapter/permission/invalid/output-validation/unknown). Drives
    /// the Doctor `runs.recovery` row.
    pub fn runs_needing_operator_action(&self) -> usize {
        self.newest_run_per_task()
            .into_iter()
            .filter(|run| {
                run.status == RunStatus::Failed
                    && run
                        .failure_class
                        .map(|c| c.needs_operator_action())
                        .unwrap_or(false)
            })
            .count()
    }

    /// Count of tasks whose newest run failed transiently and has a scheduled
    /// (not-yet-exhausted) bounded retry pending. Drives the Doctor `runs.recovery`
    /// row's informational note.
    pub fn runs_retry_pending(&self) -> usize {
        self.newest_run_per_task()
            .into_iter()
            .filter(|run| {
                run.status == RunStatus::Failed
                    && run
                        .retry
                        .as_ref()
                        .map(|r| !r.exhausted && r.not_before_secs.is_some())
                        .unwrap_or(false)
            })
            .count()
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

    /// The bounded, redacted **run-log tail** for one run, optionally only the
    /// lines strictly AFTER `since` (an exclusive 1-based sequence cursor) — the
    /// pollable analogue of Paperclip's byte-`offset` read
    /// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10). A run with no captured log
    /// (the deterministic local-echo path, or a run that has not executed)
    /// returns an EMPTY [`relux_core::RunLog`] — never an error — so the UI never
    /// blanks. The run-level truncation/dropped markers survive an incremental
    /// (`since`) fetch.
    pub fn run_log(&self, run_id: &RunId, since: Option<u32>) -> relux_core::RunLog {
        match self.run_logs.get(run_id) {
            Some(log) => log.since(since),
            None => relux_core::RunLog::empty(run_id.clone()),
        }
    }

    /// Whether a CANONICAL (finalized, persisted) run-log tail exists for `run_id`.
    /// The HTTP `get_run_logs` handler uses this to decide precedence: once the
    /// durable log exists it wins; until then an in-flight run is served from the
    /// in-memory live registry (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10).
    pub fn has_run_log(&self, run_id: &RunId) -> bool {
        self.run_logs.contains_key(run_id)
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

/// The current real wall-clock instant in unix seconds.
///
/// The kernel's logical [`Clock`] is deliberately NOT wall-clock (it orders
/// events reproducibly). But the bounded transient-retry backoff
/// (`[2m,10m,30m,2h]`, `relux_core::run_failure`) is a GENUINELY time-based
/// feature, so — exactly like `auth.rs` session expiry — it reads real time. This
/// is the only honest representation of a real backoff: a logical-tick "deadline"
/// would advance per kernel operation, not per second. The retry-state math stays
/// pure (it takes `now_secs` as input); only this one read touches the OS clock.
fn real_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

    /// The run id this prepared brief executes under (already started + stamped in
    /// `prepare_orchestration_round`). Used by the off-lock streaming driver to key
    /// the live run-log buffer so a poll can find it WHILE the process runs.
    pub fn run_id(&self) -> &RunId {
        &self.plan.run_id
    }

    /// Run the prepared adapter process. NO kernel access — pure blocking I/O on
    /// the already-resolved, redaction-ready spec, safe to call on a worker thread
    /// while the kernel lock is released. The returned [`FinishedBrief`] is merged
    /// back into the orchestration record under the lock by
    /// [`KernelState::finalize_prepared_brief`].
    pub fn run(self) -> FinishedBrief {
        self.run_with_sink(None)
    }

    /// Like [`Self::run`] but additionally streams the adapter's stdout/stderr
    /// chunks to an optional live [`crate::live_run_log::RunLogSink`] as they are
    /// read, so a poll of the run's logs sees lines BEFORE it finalizes
    /// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10). The captured outcome is
    /// unchanged — streaming is strictly additive.
    pub fn run_with_sink(self, sink: Option<crate::live_run_log::RunLogSink>) -> FinishedBrief {
        self.run_with_sink_cancellable(sink, None)
    }

    /// Like [`Self::run_with_sink`] but additionally honours an optional
    /// [`crate::run_cancel::CancelToken`]: if an operator requests cancellation
    /// mid-flight the spawn kills the child and the outcome is marked `cancelled`,
    /// which `finalize_cli_run` records as a
    /// [`relux_core::RunFailureClass::Cancelled`] run
    /// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26). Strictly additive — with
    /// `cancel: None` this is exactly [`Self::run_with_sink`].
    pub fn run_with_sink_cancellable(
        self,
        sink: Option<crate::live_run_log::RunLogSink>,
        cancel: Option<crate::run_cancel::CancelToken>,
    ) -> FinishedBrief {
        let outcome =
            crate::adapter::run_adapter_command_streaming_cancellable(&self.plan.spec, sink, cancel);
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

/// Like [`run_briefs_in_parallel`] but each brief STREAMS its stdout/stderr lines
/// to the shared [`crate::live_run_log::LiveRunLogs`] registry as they are read, so
/// a poll of `GET /v1/relux/runs/:id/logs` shows lines while the briefs run
/// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§10 — the LIVE run-log seam). This is
/// the off-lock driver the HTTP server's non-blocking job uses (the kernel lock is
/// released during this call, so the live reads are unblocked). A live buffer is
/// opened per brief before its process starts; the caller drops it via
/// [`crate::live_run_log::LiveRunLogs::finish`] after the brief's canonical log is
/// finalized + persisted. The captured outcomes are identical to the non-streaming
/// driver — streaming is strictly additive.
/// Each brief additionally opens a [`crate::run_cancel::CancelToken`] in the shared
/// `cancels` registry before its process starts, so an operator can kill it
/// mid-flight via `POST /v1/relux/runs/:id/cancel` while the kernel lock is free
/// (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26). The caller drops both the live
/// buffer and the cancel token via `finish` after the brief finalizes.
pub fn run_briefs_in_parallel_streaming(
    prepared: Vec<PreparedBrief>,
    live: &crate::live_run_log::LiveRunLogs,
    cancels: &crate::run_cancel::RunCancellations,
) -> Vec<FinishedBrief> {
    if prepared.len() <= 1 {
        return prepared
            .into_iter()
            .map(|p| {
                let sink = live.begin(p.run_id());
                let cancel = cancels.begin(p.run_id());
                p.run_with_sink_cancellable(Some(sink), Some(cancel))
            })
            .collect();
    }
    let handles: Vec<_> = prepared
        .into_iter()
        .map(|p| {
            let sink = live.begin(p.run_id());
            let cancel = cancels.begin(p.run_id());
            std::thread::spawn(move || p.run_with_sink_cancellable(Some(sink), Some(cancel)))
        })
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

/// Per-collection cap on a [`KernelState::context_snapshot`], bounding the clone so a large board
/// cannot blow up memory. The read-only list tools further bound what they render.
const MAX_SNAPSHOT_ITEMS: usize = 100;

/// Lift a short, sanitized one-line human detail from a task's `input` JSON for the read-only
/// `get_task` tool, or `None` when there is no readable detail. NEVER returns the raw JSON: a
/// string input is taken directly, an object is probed for the common human-text keys, and the
/// result is control-char-stripped, whitespace-collapsed, and length-bounded. Pure.
fn task_detail_line(input: &serde_json::Value) -> Option<String> {
    const MAX_DETAIL_CHARS: usize = 240;
    let raw = match input {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => ["description", "details", "goal", "prompt", "summary"]
            .iter()
            .find_map(|k| map.get(*k).and_then(|v| v.as_str()).map(|s| s.to_string())),
        _ => None,
    }?;
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_DETAIL_CHARS)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// The `snake_case` wire label for a run status (matching the serialized form), for the read-only
/// `list_runs` tool. Pure.
fn run_status_label(status: &RunStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Max chars kept from a redacted free-text field (a run summary/error, an approval action/reason)
/// projected into the read-only context snapshot. Keeps a large body bounded before the per-result
/// clamp; the value is short so a verbose adapter summary cannot blow the snapshot.
const MAX_REDACTED_CHARS: usize = 240;

/// Max chars kept from a short redacted id-shaped field (an approval requester).
const MAX_ARG_REDACTED_CHARS: usize = 80;

/// Redact + bound a free-text string for the read-only context snapshot: strip control chars,
/// collapse whitespace, and clamp to `max`. Returns `None` when nothing readable remains, so an
/// empty/whitespace-only field is projected as absent rather than a blank. Pure. The fields this is
/// applied to are already human-readable renderings (no secret/token), but redacting here keeps the
/// snapshot bounded and control-char-free regardless of what an adapter emitted.
fn redact_line(s: &str, max: usize) -> Option<String> {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// The label for a plugin kind, for the read-only `list_plugins` tool. Pure.
fn plugin_kind_label(kind: &PluginKind) -> String {
    format!("{kind:?}")
}

/// The `snake_case` wire label for an approval status (matching the serialized form), for the
/// read-only `list_approvals` tool. Pure.
fn approval_status_label(status: &ApprovalStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Lowercase hex SHA-256 of `bytes`. Used to bind a per-tool-call approval to the
/// exact arguments snapshot (and to re-check that snapshot before execution). Pure.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The maximum characters kept in a per-tool-call approval's args preview. Bounds
/// what the Approvals page renders regardless of the (already size-capped) args.
const MAX_ARGS_PREVIEW_CHARS: usize = 1_000;

/// Build a bounded, secret-redacted preview of a tool invocation's arguments for
/// the Approvals page. The raw snapshot is stored for execution; this is the only
/// thing the operator-facing surface renders, so any value under a secret-looking
/// key (token/password/secret/api-key/authorization/…) is masked and the whole
/// preview is length-clamped. Pure; mirrors Hermes's "sanitize + clamp every
/// model/operator-facing string" discipline (`message_sanitization.py`).
fn redact_args_for_preview(input: &serde_json::Value) -> String {
    let redacted = redact_secret_values(input);
    let rendered = serde_json::to_string_pretty(&redacted)
        .unwrap_or_else(|_| "<unrenderable arguments>".to_string());
    if rendered.chars().count() > MAX_ARGS_PREVIEW_CHARS {
        let head: String = rendered.chars().take(MAX_ARGS_PREVIEW_CHARS).collect();
        format!("{head}… (truncated)")
    } else {
        rendered
    }
}

/// Whether an object key looks like it names a secret, so its value is masked in a
/// preview. Conservative substring match on a lowercased key.
fn key_looks_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "token",
        "password",
        "passwd",
        "secret",
        "api_key",
        "apikey",
        "api-key",
        "authorization",
        "auth",
        "credential",
        "private_key",
        "access_key",
        "session",
    ];
    NEEDLES.iter().any(|n| k.contains(n))
}

/// Recursively mask the values of secret-looking keys in a JSON value, preserving
/// structure so the operator can still see the shape of the arguments.
fn redact_secret_values(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if key_looks_secret(k) && !v.is_null() {
                    out.insert(k.clone(), serde_json::Value::String("***redacted***".to_string()));
                } else {
                    out.insert(k.clone(), redact_secret_values(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_secret_values).collect())
        }
        other => other.clone(),
    }
}

/// The `snake_case` wire label for a risk level (matching the serialized form), for the read-only
/// `list_approvals` tool. Pure.
fn risk_level_label(risk: &RiskLevel) -> String {
    serde_json::to_value(risk)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
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

    // Hermes-first contextual conversation chips. The work CTAs are SUPPRESSED on
    // casual / emotional turns (below); here we replace them with NON-action chips
    // that route ordinary read-only / conversational messages — never a task, plan,
    // or run. Each chip's `message` is something the user could type by hand and is
    // itself a conversational or read-only ask, so clicking it can do nothing the
    // user could not do in chat (`docs/prime-processing-audit.md` "Hermes-first
    // general agent"; §10.5, §11.1, §17.1).
    //
    // Venting / frustration: offer to dig into what actually went wrong (an
    // explanation) or surface the live state (a read-only status) — not a work CTA.
    if turn.intent == PrimeIntent::EmotionalSupport {
        turn.suggested_actions.push(PrimeSuggestion {
            label: "Tell me what broke".to_string(),
            message: "what went wrong?".to_string(),
            send: false,
        });
        turn.suggested_actions.push(PrimeSuggestion {
            label: "Show me the last run".to_string(),
            message: "what is going on?".to_string(),
            send: true,
        });
    }
    // A greeting or light chitchat: at most a single discovery chip so the user can
    // learn what Prime can do — no work prompt, nothing created.
    if turn.intent == PrimeIntent::Greeting || turn.intent == PrimeIntent::SmallTalk {
        turn.suggested_actions.push(PrimeSuggestion {
            label: "What can you do?".to_string(),
            message: "what tools can you use?".to_string(),
            send: true,
        });
    }

    // Brainstorming stays a conversation (§10.5), but give the user a one-click
    // path to promote the idea into a task. The button pre-fills the command with
    // the work the message gestured at (`send: false`) so the user confirms or
    // edits the title - nothing is created until they send it.
    //
    // Hermes-first suggestion policy: the work CTAs appear ONLY when the message
    // gestured at REAL, nameable work. A vent, an insult, or empty small talk that
    // the brain happened to label `Brainstorming` ("fuck you", "ugh", "lol") gets a
    // normal conversational reply with NO "Turn this into a task" / "Plan this out"
    // buttons — offering those there is absurd. The gate is deterministic and
    // presentation-only ([`crate::prime::brainstorm_offers_actionable_work`]); it
    // creates and runs nothing (`docs/prime-processing-audit.md` "Hermes-first
    // general agent"; §10.5, §11.1, §17.1).
    if turn.intent == PrimeIntent::Brainstorming
        && crate::prime::brainstorm_offers_actionable_work(message)
    {
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
        let multi = plan.is_multi_agent();
        let suggestion = if multi {
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

        // Attach the reviewable plan preview as STRUCTURED data so the dashboard can
        // render a card instead of parsing the prose reply (§10 planning layer,
        // §11.1). It is built from the SAME `plan` the commit suggestion is keyed on,
        // so the card shows exactly what "Create these tasks" would create. The
        // proposal carries no action - it is informational only; the explicit
        // suggestion above is the lone commit path (§10.5, §17.1). A single-step goal
        // gets an empty-step proposal so the card can still name the goal and the
        // one-task route honestly, without inventing a fan-out.
        let steps: Vec<relux_core::PrimeProposalStep> = if multi {
            plan.steps
                .iter()
                .enumerate()
                .map(|(i, s)| relux_core::PrimeProposalStep {
                    index: (i + 1) as u32,
                    title: s.title.clone(),
                    role: s.role.label().to_string(),
                    agent: s.agent_id.clone().unwrap_or_else(|| "prime".to_string()),
                })
                .collect()
        } else {
            Vec::new()
        };
        let agents = if multi { plan.agent_labels() } else { Vec::new() };
        turn.proposal = Some(relux_core::PrimeProposal {
            goal,
            multi_step: multi,
            steps,
            agents,
            // The deterministic proposal carries NO polish. When the OpenRouter
            // brain is enabled, the server layers an advisory presentation overlay
            // on top OUTSIDE the lock (see `polish_proposal`); it never changes the
            // authoritative steps/agents/goal built here (§10 planning layer, §17.1).
            polish: None,
        });
    }
}

/// Render a `PrimeAction` as a one-line human-readable string for approvals and
/// audit metadata.
/// The bundle of OPTIONAL brain-proposed slots handed to
/// [`KernelState::prime_turn_with_brain`], one per action the brain can sharpen.
///
/// Every field is `None` on the deterministic path; each is reconciled and validated
/// at the single kernel chokepoint before it can shape anything. Defaulting to all
/// `None` keeps the simple call sites byte-for-byte deterministic.
#[derive(Default)]
pub struct BrainSlotProposals<'a> {
    /// Task slots for a `TaskCreation` / `CreateAndRunTask` turn.
    pub task: Option<&'a crate::prime_slots::BrainTaskSlots>,
    /// Agent slots for an `AgentCreation` turn.
    pub agent: Option<&'a crate::prime_agent_slots::BrainAgentSlots>,
    /// Advisory plugin reference for a `PluginInstallation` `Propose` turn.
    pub plugin: Option<&'a crate::prime_admin_slots::BrainPluginRef>,
    /// Advisory permission subject for a `PermissionChange` `Propose` turn.
    pub permission: Option<&'a crate::prime_admin_slots::BrainPermissionSlots>,
    /// Resolved assignment slots for an `AssignTask` turn the deterministic extractors
    /// could not complete (validated against the live state before promoting to an Act).
    pub assign: Option<&'a crate::prime_assign_slots::BrainAssignSlots>,
    /// Resolved by-id update slots for a `TaskUpdate` turn the deterministic rail could
    /// not resolve (validated against the live state before promoting to an Act).
    pub update: Option<&'a crate::prime_update_slots::BrainUpdateSlots>,
    /// A `task.start` write-tool reference for a `RunStart` turn the deterministic path could
    /// not complete (the message named no ready task id). Validated against the live ready
    /// queue before promoting to the SAME safe `StartRun` action.
    pub run: Option<&'a crate::prime_write_tools::BrainRunStart>,
    /// An `orchestration.create` write-tool goal for an `Orchestration` turn. The validated
    /// goal REPLACES the keyword-sliced goal that flows into the EXISTING `OrchestrateGoal`
    /// action; the deterministic planner still owns the decomposition, agent grounding, the
    /// step cap, the DAG, and the multi-agent gate (a goal that does not split is dropped).
    pub orchestration: Option<&'a crate::prime_orchestration_slots::BrainOrchestrationSlots>,
    /// An `orchestration.start` write-tool reference for an `OrchestrationRun` turn the
    /// deterministic path could not complete (the message named no id). Validated against the
    /// live orchestration records (it must EXIST with at least one pending brief) before
    /// promoting to the SAME safe `RunOrchestration` action.
    pub run_orchestration: Option<&'a crate::prime_write_tools::BrainRunOrchestration>,
    /// Whether this bundle was computed by the caller on the COMBINED message of a
    /// multi-turn *continuation* (vs. the raw message of a fresh turn). The kernel keeps
    /// the bundle only when this matches the turn it actually produced — continuation
    /// slots are valid ONLY on a continuation, raw slots ONLY on a fresh turn — so a
    /// proposal computed for the wrong message can never shape an action.
    pub continuation: bool,
}

/// Sharpen a risky, approval-gated admin action (`InstallPlugin` / `GrantPermission`)
/// with a validated brain proposal, returning the (possibly reshaped) action, the
/// updated human-readable text, and the advisory provenance to surface.
///
/// This is advisory only: the returned action is ALWAYS still proposed behind a human
/// approval by the caller. The brain can never execute an install or a grant — it only
/// sharpens the subject the human reviews. On no/low-confidence/unvalidated proposal
/// the action is returned unchanged with `None` provenance (the deterministic subject
/// stands). A permission subject is honored ONLY when it names an EXISTING agent; a
/// plugin id is normalized; neither can invent capability.
fn sharpen_admin_action(
    action: PrimeAction,
    text: String,
    slots: &BrainSlotProposals<'_>,
    summary: &relux_core::StateSummary,
) -> (PrimeAction, String, Option<relux_core::PrimeAdminSlots>) {
    match action {
        PrimeAction::InstallPlugin { plugin_id } => {
            if let Some(proposal) = slots.plugin {
                if let Some(sharpened) =
                    crate::prime_admin_slots::reconcile_plugin_ref(&plugin_id, proposal)
                {
                    let text = format!("I can install the plugin {sharpened}.");
                    let admin = relux_core::PrimeAdminSlots {
                        kind: "plugin_install".to_string(),
                        plugin_id: Some(sharpened.clone()),
                        subject_kind: None,
                        subject_id: None,
                        permission: None,
                        source: None,
                    };
                    return (PrimeAction::InstallPlugin { plugin_id: sharpened }, text, Some(admin));
                }
            }
            (PrimeAction::InstallPlugin { plugin_id }, text, None)
        }
        PrimeAction::GrantPermission {
            subject_id,
            permission,
        } => {
            if let Some(proposal) = slots.permission {
                if let Some(sharpened) =
                    crate::prime_admin_slots::reconcile_permission_slots(proposal, summary)
                {
                    // Keep the deterministic permission label unless the brain offered
                    // a (sanitized) one of its own.
                    let permission = sharpened.permission.clone().unwrap_or(permission);
                    let text = format!("I can grant {permission} to {}.", sharpened.subject_id);
                    let admin = relux_core::PrimeAdminSlots {
                        kind: "permission_grant".to_string(),
                        plugin_id: None,
                        subject_kind: Some(sharpened.subject_kind.clone()),
                        subject_id: Some(sharpened.subject_id.clone()),
                        permission: Some(permission.clone()),
                        source: None,
                    };
                    return (
                        PrimeAction::GrantPermission {
                            subject_id: sharpened.subject_id,
                            permission,
                        },
                        text,
                        Some(admin),
                    );
                }
            }
            (PrimeAction::GrantPermission { subject_id, permission }, text, None)
        }
        other => (other, text, None),
    }
}

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
        PrimeAction::RunOrchestration { orchestration_id } => {
            format!("run orchestration {orchestration_id}")
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
            ToolExecutability::NeedsApproval => "needs approval (higher-risk tool)",
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

    // --- Operator-configured plugin tools ----------------------------------

    /// A generated metadata-only wrapper (no tools) installed as a user plugin,
    /// mirroring `plugin_install::scaffold_manifest`.
    fn wrapper_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: PluginId::new(id),
            name: format!("{id} (metadata only)"),
            version: "0.0.0".to_string(),
            kind: PluginKind::ToolSet,
            description: "Installed as metadata: no runnable tools yet.".to_string(),
            author: crate::plugin_install::GENERATED_MANIFEST_AUTHOR.to_string(),
            trust_level: TrustLevel::Unverified,
            capabilities: PluginCapability {
                tools: Vec::new(),
                permissions: Vec::new(),
            },
            health: PluginHealth::Unknown,
        }
    }

    fn primed_with_wrapper() -> (KernelState, AgentId, PluginId) {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        let id = PluginId::new("relux-plugin-my-repo");
        k.install_plugin(
            wrapper_manifest("relux-plugin-my-repo"),
            PluginSourceKind::LocalDir,
            "/tmp/my-repo".to_string(),
            "/data/my-repo".to_string(),
            true,
        );
        (k, prime, id)
    }

    fn tool_input(json: &str) -> crate::plugin_tool_config::PluginToolInput {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        crate::plugin_tool_config::parse_plugin_tool_input(&v).unwrap()
    }

    #[test]
    fn configure_plugin_tool_adds_a_validated_tool_to_a_wrapper() {
        let (mut k, prime, id) = primed_with_wrapper();
        // The wrapper starts with no tools at all.
        assert!(k.plugin(&id).unwrap().capabilities.tools.is_empty());

        let def = k
            .configure_plugin_tool(&id, tool_input(r#"{"name":"report.fetch","risk":"low"}"#))
            .expect("configure tool");
        // Permission is DERIVED, never operator-supplied.
        assert_eq!(def.permission.as_str(), "tool:relux-plugin-my-repo:fetch");
        assert_eq!(def.approval, ApprovalRequirement::Never);

        // The manifest now declares the tool + its permission.
        let manifest = k.plugin(&id).unwrap();
        assert_eq!(manifest.capabilities.tools.len(), 1);
        assert_eq!(manifest.capabilities.tools[0].name, "report.fetch");
        assert!(manifest
            .capabilities
            .permissions
            .iter()
            .any(|p| p.as_str() == "tool:relux-plugin-my-repo:fetch"));

        // Discovery: with no runtime it is honestly not-yet-runnable.
        let tools = k.discover_tools(None);
        let t = tools.iter().find(|t| t.tool_name == "report.fetch").unwrap();
        assert_eq!(t.executable, ToolExecutability::RuntimeNotConfigured);

        // After enabling a runtime + granting the permission it becomes Ready and
        // is actually invocable through the loopback path.
        let base = one_shot_http(
            "HTTP/1.1 200 OK\r\nContent-Length: 24\r\nConnection: close\r\n\r\n{\"output\":{\"ok\":true}}\n\n\n\n",
        );
        k.configure_tool_runtime(&id, &base, true, Some(2_000)).unwrap();
        k.grant_permission_to_agent(&prime, def.permission.clone()).unwrap();
        let t = k
            .discover_tools(Some(&prime))
            .into_iter()
            .find(|t| t.tool_name == "report.fetch")
            .unwrap();
        assert_eq!(t.executable, ToolExecutability::Ready);
        let result = k
            .invoke_tool(&prime, &id, "report.fetch", serde_json::json!({}))
            .expect("invocable");
        assert_eq!(result.output, serde_json::json!({ "ok": true }));
    }

    #[test]
    fn configure_plugin_tool_upserts_by_name_and_persists_across_snapshot() {
        let (mut k, _prime, id) = primed_with_wrapper();
        k.configure_plugin_tool(&id, tool_input(r#"{"name":"report.fetch","description":"v1"}"#))
            .unwrap();
        k.configure_plugin_tool(&id, tool_input(r#"{"name":"report.fetch","description":"v2"}"#))
            .unwrap();
        // Upsert, not duplicate.
        assert_eq!(k.plugin(&id).unwrap().capabilities.tools.len(), 1);
        assert_eq!(k.plugin(&id).unwrap().capabilities.tools[0].description, "v2");

        // The mutated manifest survives a snapshot roundtrip (the store is
        // authoritative for a user plugin).
        let restored = KernelState::from_snapshot(k.snapshot());
        assert_eq!(restored.plugin(&id).unwrap().capabilities.tools.len(), 1);
    }

    #[test]
    fn a_non_low_risk_tool_needs_approval_and_is_refused_directly() {
        let (mut k, prime, id) = primed_with_wrapper();
        let def = k
            .configure_plugin_tool(&id, tool_input(r#"{"name":"deploy.run","risk":"high"}"#))
            .unwrap();
        // A risky tool is approval-Required regardless of any auto-approve attempt.
        assert_eq!(def.approval, ApprovalRequirement::Required);

        // Even with a runtime enabled and the permission held, it is gated.
        k.configure_tool_runtime(&id, "http://127.0.0.1:19999", true, None).unwrap();
        k.grant_permission_to_agent(&prime, def.permission.clone()).unwrap();
        let t = k
            .discover_tools(Some(&prime))
            .into_iter()
            .find(|t| t.tool_name == "deploy.run")
            .unwrap();
        assert_eq!(t.executable, ToolExecutability::NeedsApproval);

        let err = k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRequiresApproval { .. }), "got {err:?}");
    }

    /// Configure a high-risk `deploy.run`, grant Prime its derived permission, and
    /// return `(kernel, prime, plugin_id)` ready for the per-call approval flow.
    fn primed_with_gated_tool() -> (KernelState, AgentId, PluginId) {
        let (mut k, prime, id) = primed_with_wrapper();
        let def = k
            .configure_plugin_tool(&id, tool_input(r#"{"name":"deploy.run","risk":"high"}"#))
            .unwrap();
        k.grant_permission_to_agent(&prime, def.permission.clone()).unwrap();
        (k, prime, id)
    }

    #[test]
    fn per_call_approval_request_creates_a_bound_pending_approval() {
        let (mut k, prime, id) = primed_with_gated_tool();
        let appr_id = k
            .request_tool_invocation_approval(
                "operator",
                &prime,
                &id,
                "deploy.run",
                serde_json::json!({ "env": "prod" }),
            )
            .expect("request approval");

        // The generic approval is Pending and carries the risk/action.
        let appr = k.approval(&appr_id).unwrap();
        assert_eq!(appr.status, ApprovalStatus::Pending);
        assert_eq!(appr.risk, RiskLevel::High);
        assert!(appr.action.contains("deploy.run"));

        // The binding pins the exact invocation and is not yet consumed.
        let binding = k.pending_tool_invocation(&appr_id).unwrap();
        assert_eq!(binding.plugin_id.as_str(), id.as_str());
        assert_eq!(binding.tool_name, "deploy.run");
        assert_eq!(binding.agent_id.as_str(), "prime");
        assert_eq!(binding.permission, "tool:relux-plugin-my-repo:run");
        assert_eq!(binding.input, serde_json::json!({ "env": "prod" }));
        assert!(!binding.consumed);
        assert_eq!(binding.args_sha256.len(), 64);

        // Audited as a request.
        assert!(k.audit_log().iter().any(|e| e.action == "tool_invocation:request"
            && e.result == AuditResult::Success
            && e.metadata["tool"] == "deploy.run"));
    }

    #[test]
    fn per_call_approval_executes_once_after_approval_then_is_consumed() {
        let (mut k, prime, id) = primed_with_gated_tool();
        // A runtime must back the tool for the approved call to actually run.
        let base = one_shot_http(
            "HTTP/1.1 200 OK\r\nContent-Length: 26\r\nConnection: close\r\n\r\n{\"output\":{\"deployed\":1}}\n\n",
        );
        k.configure_tool_runtime(&id, &base, true, Some(2_000)).unwrap();

        let appr_id = k
            .request_tool_invocation_approval(
                "operator",
                &prime,
                &id,
                "deploy.run",
                serde_json::json!({ "env": "prod" }),
            )
            .unwrap();

        // Cannot execute while still Pending.
        let err = k
            .execute_approved_tool_invocation(&appr_id, "operator")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolInvocationNotApproved { .. }), "got {err:?}");

        // Approve, then execute once.
        k.resolve_approval(&appr_id, true, "operator", None).unwrap();
        let result = k
            .execute_approved_tool_invocation(&appr_id, "operator")
            .expect("approved invocation runs");
        assert_eq!(result.output, serde_json::json!({ "deployed": 1 }));
        assert_eq!(result.tool_name, "deploy.run");

        // One-shot: the binding is consumed and a second execute is refused without
        // dialing the (now closed) loopback server.
        assert!(k.pending_tool_invocation(&appr_id).unwrap().consumed);
        let err = k
            .execute_approved_tool_invocation(&appr_id, "operator")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolInvocationConsumed(_)), "got {err:?}");

        // Audited as an execution success bound to the approval.
        assert!(k.audit_log().iter().any(|e| e.action == "tool:relux-plugin-my-repo:run"
            && e.result == AuditResult::Success
            && e.metadata["via"] == "tool_invocation:execute"
            && e.metadata["approval"] == appr_id.as_str()));
    }

    #[test]
    fn a_runtime_failure_still_consumes_the_approved_invocation() {
        let (mut k, prime, id) = primed_with_gated_tool();
        // Point at a dead loopback port so the runtime dial fails.
        k.configure_tool_runtime(&id, "http://127.0.0.1:19999", true, Some(300)).unwrap();
        let appr_id = k
            .request_tool_invocation_approval("operator", &prime, &id, "deploy.run", serde_json::json!({}))
            .unwrap();
        k.resolve_approval(&appr_id, true, "operator", None).unwrap();

        // The attempt fails at the runtime, but still consumes the binding.
        let err = k
            .execute_approved_tool_invocation(&appr_id, "operator")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRuntimeInvocation { .. }), "got {err:?}");
        assert!(k.pending_tool_invocation(&appr_id).unwrap().consumed);
        // A retry needs a fresh approval; the consumed binding refuses.
        let err = k
            .execute_approved_tool_invocation(&appr_id, "operator")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolInvocationConsumed(_)), "got {err:?}");
    }

    #[test]
    fn rejecting_a_per_call_approval_drops_the_binding() {
        let (mut k, prime, id) = primed_with_gated_tool();
        let appr_id = k
            .request_tool_invocation_approval("operator", &prime, &id, "deploy.run", serde_json::json!({}))
            .unwrap();
        k.resolve_approval(&appr_id, false, "operator", None).unwrap();
        // The binding is dropped and execution is refused (status is not Approved).
        assert!(k.pending_tool_invocation(&appr_id).is_none());
        let err = k
            .execute_approved_tool_invocation(&appr_id, "operator")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolInvocationNotApproved { .. }), "got {err:?}");
    }

    #[test]
    fn requesting_approval_for_a_directly_runnable_tool_is_refused() {
        let (mut k, prime, id) = primed_with_wrapper();
        // A low-risk tool is auto-approvable; the per-call flow is the wrong path.
        let def = k
            .configure_plugin_tool(&id, tool_input(r#"{"name":"report.fetch","risk":"low"}"#))
            .unwrap();
        k.grant_permission_to_agent(&prime, def.permission).unwrap();
        let err = k
            .request_tool_invocation_approval("operator", &prime, &id, "report.fetch", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolDoesNotRequireApproval { .. }), "got {err:?}");
    }

    #[test]
    fn requesting_approval_without_the_permission_is_denied() {
        let (mut k, _prime, id) = primed_with_wrapper();
        k.configure_plugin_tool(&id, tool_input(r#"{"name":"deploy.run","risk":"high"}"#))
            .unwrap();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let weak = k
            .create_agent("weak", "Weak", "no perms", &adapter, &ns, None, vec![])
            .unwrap();
        let err = k
            .request_tool_invocation_approval("operator", &weak, &id, "deploy.run", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");
    }

    #[test]
    fn per_call_binding_survives_a_snapshot_roundtrip() {
        let (mut k, prime, id) = primed_with_gated_tool();
        let appr_id = k
            .request_tool_invocation_approval("operator", &prime, &id, "deploy.run", serde_json::json!({ "env": "prod" }))
            .unwrap();
        let restored = KernelState::from_snapshot(k.snapshot());
        let binding = restored.pending_tool_invocation(&appr_id).expect("binding persisted");
        assert_eq!(binding.tool_name, "deploy.run");
        assert_eq!(binding.input, serde_json::json!({ "env": "prod" }));
        assert!(!binding.consumed);
    }

    #[test]
    fn secret_args_are_redacted_in_preview_but_stored_verbatim() {
        let (mut k, prime, id) = primed_with_gated_tool();
        let appr_id = k
            .request_tool_invocation_approval(
                "operator",
                &prime,
                &id,
                "deploy.run",
                serde_json::json!({ "token": "s3cr3t", "env": "prod" }),
            )
            .unwrap();
        let binding = k.pending_tool_invocation(&appr_id).unwrap();
        // The preview masks the secret-looking value but keeps the rest visible.
        assert!(binding.args_preview.contains("***redacted***"));
        assert!(!binding.args_preview.contains("s3cr3t"));
        assert!(binding.args_preview.contains("prod"));
        // The stored snapshot keeps the real value so the approved call runs verbatim.
        assert_eq!(binding.input["token"], "s3cr3t");
    }

    // --- Persistent allow-always grants ----------------------------------------

    /// `(kernel, prime, plugin_id)` with a high-risk `deploy.run` gated tool, Prime
    /// holding its permission, AND an enabled loopback runtime that echoes a fixed
    /// output — ready to prove a grant lets a direct invoke through.
    fn primed_with_gated_tool_and_runtime() -> (KernelState, AgentId, PluginId) {
        let (mut k, prime, id) = primed_with_gated_tool();
        let base = one_shot_http(
            "HTTP/1.1 200 OK\r\nContent-Length: 24\r\nConnection: close\r\n\r\n{\"output\":{\"ran\":1}}\n\n",
        );
        k.configure_tool_runtime(&id, &base, true, Some(2_000)).unwrap();
        (k, prime, id)
    }

    #[test]
    fn gated_tool_is_refused_without_a_grant_then_runs_with_one() {
        let (mut k, prime, id) = primed_with_gated_tool_and_runtime();

        // Without a grant, a direct invoke of the gated tool is refused.
        let err = k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({ "env": "prod" }))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRequiresApproval { .. }), "got {err:?}");

        // Create a standing allow-always grant for the exact (subject, plugin, tool).
        let grant = k
            .grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .expect("grant created");
        assert_eq!(grant.subject_agent.as_str(), "prime");
        assert_eq!(grant.tool_name, "deploy.run");
        assert_eq!(grant.permission, "tool:relux-plugin-my-repo:run");
        assert_eq!(grant.risk, RiskLevel::High);
        assert!(grant.last_used_at.is_none());

        // Now the same invocation bypasses the prompt and actually runs.
        let result = k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({ "env": "prod" }))
            .expect("grant bypasses the per-call prompt");
        assert_eq!(result.output, serde_json::json!({ "ran": 1 }));

        // The use is audited and the grant's last_used_at is stamped.
        assert!(k.audit_log().iter().any(|e| e.action == "grant:use"
            && e.result == AuditResult::Success
            && e.metadata["grant"] == grant.id
            && e.metadata["tool"] == "deploy.run"));
        assert!(k.persistent_grant(&grant.id).unwrap().last_used_at.is_some());
        // Creating it was audited too.
        assert!(k.audit_log().iter().any(|e| e.action == "grant:create"
            && e.metadata["tool"] == "deploy.run"));
    }

    #[test]
    fn a_grant_only_covers_its_exact_subject_plugin_and_tool() {
        let (mut k, prime, id) = primed_with_gated_tool_and_runtime();
        // A second gated tool on the same plugin.
        let other = k
            .configure_plugin_tool(&id, tool_input(r#"{"name":"deploy.destroy","risk":"high"}"#))
            .unwrap();
        k.grant_permission_to_agent(&prime, other.permission.clone()).unwrap();
        // A second agent that also holds deploy.run's permission.
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let other_agent = k
            .create_agent("worker", "Worker", "", &adapter, &ns, None, vec![])
            .unwrap();
        let run_perm = relux_core::Permission::new("tool:relux-plugin-my-repo:run").unwrap();
        k.grant_permission_to_agent(&other_agent, run_perm).unwrap();

        // Grant covers ONLY (prime, deploy.run).
        k.grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .unwrap();

        // A different TOOL is still gated.
        let err = k
            .invoke_tool(&prime, &id, "deploy.destroy", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRequiresApproval { .. }), "different tool: {err:?}");

        // A different SUBJECT is still gated, even with the same permission.
        let err = k
            .invoke_tool(&other_agent, &id, "deploy.run", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRequiresApproval { .. }), "different subject: {err:?}");
    }

    #[test]
    fn a_risk_escalation_invalidates_the_grant() {
        let (mut k, prime, id) = primed_with_gated_tool_and_runtime();
        k.grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .unwrap();
        // It runs while the risk matches.
        assert!(k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({}))
            .is_ok());

        // Re-configuring the tool to a higher risk changes the bound risk class, so
        // the grant no longer matches and the prompt is required again (fail closed).
        k.configure_plugin_tool(&id, tool_input(r#"{"name":"deploy.run","risk":"critical"}"#))
            .unwrap();
        let err = k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRequiresApproval { .. }), "got {err:?}");
    }

    #[test]
    fn revoking_a_grant_restores_the_per_call_gate() {
        let (mut k, prime, id) = primed_with_gated_tool_and_runtime();
        let grant = k
            .grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .unwrap();
        // Revoke it.
        k.revoke_persistent_grant(&grant.id, "operator").unwrap();
        assert!(k.persistent_grant(&grant.id).is_none());
        assert!(k.audit_log().iter().any(|e| e.action == "grant:revoke"
            && e.metadata["tool"] == "deploy.run"));

        // The invocation is gated again.
        let err = k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRequiresApproval { .. }), "got {err:?}");

        // Revoking an unknown grant is an honest error.
        let err = k.revoke_persistent_grant("grant_9999", "operator").unwrap_err();
        assert!(matches!(err, KernelError::UnknownPersistentGrant(_)), "got {err:?}");
    }

    #[test]
    fn the_permission_check_still_applies_after_a_grant() {
        let (mut k, prime, id) = primed_with_gated_tool_and_runtime();
        let grant = k
            .grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .unwrap();
        // Revoke Prime's underlying permission AFTER granting allow-always.
        let run_perm = relux_core::Permission::new("tool:relux-plugin-my-repo:run").unwrap();
        k.revoke_permission_from_agent(&prime, &run_perm).unwrap();

        // The grant bypasses ONLY the prompt; the permission check still denies.
        let err = k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");
        // The grant row is untouched (it still exists; permission is the separate gate).
        assert!(k.persistent_grant(&grant.id).is_some());
    }

    #[test]
    fn granting_a_directly_runnable_tool_is_refused() {
        let (mut k, prime, id) = primed_with_wrapper();
        let def = k
            .configure_plugin_tool(&id, tool_input(r#"{"name":"report.fetch","risk":"low"}"#))
            .unwrap();
        k.grant_permission_to_agent(&prime, def.permission).unwrap();
        // A low-risk auto-approve tool needs no grant — refused (a grant is meaningless).
        let err = k
            .grant_persistent_tool_invocation("operator", &prime, &id, "report.fetch")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolDoesNotRequireApproval { .. }), "got {err:?}");
    }

    #[test]
    fn granting_without_the_permission_or_for_unknown_tool_is_refused() {
        let (mut k, _prime, id) = primed_with_gated_tool();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let weak = k
            .create_agent("weak", "Weak", "no perms", &adapter, &ns, None, vec![])
            .unwrap();
        // Subject lacks the permission → denied (no boundary widening).
        let err = k
            .grant_persistent_tool_invocation("operator", &weak, &id, "deploy.run")
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");
        // An unknown tool cannot be granted (malformed grant rejected).
        let err = k
            .grant_persistent_tool_invocation("operator", &weak, &id, "nope.missing")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolNotFound { .. }), "got {err:?}");
    }

    #[test]
    fn creating_an_identical_grant_is_idempotent() {
        let (mut k, prime, id) = primed_with_gated_tool();
        let a = k
            .grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .unwrap();
        let b = k
            .grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .unwrap();
        // Same row returned; no duplicate.
        assert_eq!(a.id, b.id);
        assert_eq!(k.persistent_grants().len(), 1);
    }

    #[test]
    fn allow_always_from_approval_approves_and_persists() {
        let (mut k, prime, id) = primed_with_gated_tool_and_runtime();
        let appr_id = k
            .request_tool_invocation_approval(
                "operator",
                &prime,
                &id,
                "deploy.run",
                serde_json::json!({ "env": "prod" }),
            )
            .unwrap();
        // "Allow always": approves this pending approval AND creates a standing grant.
        let grant = k.allow_always_from_approval(&appr_id, "operator").unwrap();
        assert_eq!(grant.tool_name, "deploy.run");
        // The pending approval is approved (so its bound one-shot can still run).
        assert_eq!(k.approval(&appr_id).unwrap().status, ApprovalStatus::Approved);
        assert!(!k.pending_tool_invocation(&appr_id).unwrap().consumed);

        // And a FUTURE direct invoke of the same tool now bypasses the prompt via the
        // grant and actually runs (the one-shot fixture serves this single request).
        let result = k
            .invoke_tool(&prime, &id, "deploy.run", serde_json::json!({ "env": "prod" }))
            .expect("grant bypasses the prompt on a later call");
        assert_eq!(result.output, serde_json::json!({ "ran": 1 }));
    }

    #[test]
    fn allow_always_requires_a_tool_invocation_binding() {
        let mut k = KernelState::new();
        // A generic approval (no bound tool invocation) cannot be "allow always".
        let appr_id = k.request_approval("operator", "do thing", "because", RiskLevel::High, None);
        let err = k.allow_always_from_approval(&appr_id, "operator").unwrap_err();
        assert!(matches!(err, KernelError::NoBoundToolInvocation(_)), "got {err:?}");
    }

    #[test]
    fn grants_survive_a_snapshot_roundtrip() {
        let (mut k, prime, id) = primed_with_gated_tool();
        let grant = k
            .grant_persistent_tool_invocation("operator", &prime, &id, "deploy.run")
            .unwrap();
        let restored = KernelState::from_snapshot(k.snapshot());
        let g = restored.persistent_grant(&grant.id).expect("grant persisted");
        assert_eq!(g.tool_name, "deploy.run");
        assert_eq!(g.subject_agent.as_str(), "prime");
        assert_eq!(g.risk, RiskLevel::High);
    }

    #[test]
    fn configure_plugin_tool_refuses_bundled_and_unknown_plugins() {
        let (mut k, _prime, _id) = primed_with_wrapper();
        // The bundled echo fixture cannot have tools configured this way.
        k.install_plugin(
            echo_manifest(),
            PluginSourceKind::Bundled,
            "bundled".to_string(),
            "examples/relux-plugins/relux-tools-echo".to_string(),
            true,
        );
        let err = k
            .configure_plugin_tool(
                &PluginId::new("relux-tools-echo"),
                tool_input(r#"{"name":"x.run"}"#),
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::BundledPluginProtected(_)), "got {err:?}");

        // An uninstalled plugin is a clear not-installed error.
        let err = k
            .configure_plugin_tool(
                &PluginId::new("relux-plugin-nope"),
                tool_input(r#"{"name":"x.run"}"#),
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::PluginNotInstalled(_)), "got {err:?}");
    }

    #[test]
    fn remove_plugin_tool_drops_the_tool_and_its_unused_permission() {
        let (mut k, _prime, id) = primed_with_wrapper();
        let def = k
            .configure_plugin_tool(&id, tool_input(r#"{"name":"report.fetch"}"#))
            .unwrap();
        k.remove_plugin_tool(&id, "report.fetch").unwrap();
        let manifest = k.plugin(&id).unwrap();
        assert!(manifest.capabilities.tools.is_empty());
        assert!(!manifest
            .capabilities
            .permissions
            .iter()
            .any(|p| p.matches_exact(&def.permission)));

        // Removing a tool that is not there is a clear error.
        let err = k.remove_plugin_tool(&id, "nope.run").unwrap_err();
        assert!(matches!(err, KernelError::PluginToolNotFound { .. }), "got {err:?}");
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
    fn mcp_server_register_list_enable_remove() {
        let mut k = KernelState::new();
        // Register a loopback server.
        let cfg = k
            .register_mcp_server(
                "fs-helper",
                "http://127.0.0.1:8000/mcp",
                "local fs",
                true,
                None,
            )
            .expect("register ok");
        assert_eq!(cfg.id, "fs-helper");
        assert_eq!(cfg.transport.as_str(), "http_loopback");
        assert_eq!(cfg.timeout_ms, relux_core::DEFAULT_MCP_TIMEOUT_MS);
        assert_eq!(k.mcp_servers().len(), 1);

        // Disable, then re-enable.
        let off = k.set_mcp_server_enabled("fs-helper", false).unwrap();
        assert!(!off.enabled);
        assert_eq!(off.status_str(), "disabled");
        let on = k.set_mcp_server_enabled("fs-helper", true).unwrap();
        assert!(on.enabled);

        // Upsert by id (re-register updates the endpoint in place, not a 2nd row).
        k.register_mcp_server("fs-helper", "http://127.0.0.1:9001/mcp", "moved", true, Some(2000))
            .unwrap();
        assert_eq!(k.mcp_servers().len(), 1);
        let s = k.mcp_server("fs-helper").unwrap();
        assert_eq!(s.endpoint, "http://127.0.0.1:9001/mcp");
        assert_eq!(s.timeout_ms, 2000);

        // Remove.
        k.remove_mcp_server("fs-helper").unwrap();
        assert!(k.mcp_server("fs-helper").is_none());
        assert!(matches!(
            k.remove_mcp_server("fs-helper"),
            Err(KernelError::UnknownMcpServer(_))
        ));
        assert!(matches!(
            k.set_mcp_server_enabled("nope", true),
            Err(KernelError::UnknownMcpServer(_))
        ));
    }

    #[test]
    fn mcp_server_rejects_non_loopback_and_bad_id() {
        let mut k = KernelState::new();
        // A remote / https endpoint is refused (loopback-only).
        assert!(matches!(
            k.register_mcp_server("s", "https://mcp.example.com", "x", true, None),
            Err(KernelError::InvalidMcpConfig { .. })
        ));
        assert!(matches!(
            k.register_mcp_server("s", "http://10.0.0.5:8000", "x", true, None),
            Err(KernelError::InvalidMcpConfig { .. })
        ));
        // A bad id is refused.
        assert!(matches!(
            k.register_mcp_server("has space", "http://127.0.0.1:8000", "x", true, None),
            Err(KernelError::InvalidMcpConfig { .. })
        ));
        assert_eq!(k.mcp_servers().len(), 0);
    }

    #[test]
    fn mcp_discovery_disabled_and_unknown_are_honest() {
        let mut k = KernelState::new();
        assert!(matches!(
            k.discover_mcp_tools("nope"),
            Err(KernelError::UnknownMcpServer(_))
        ));
        k.register_mcp_server("s", "http://127.0.0.1:8000/mcp", "x", false, None)
            .unwrap();
        assert!(matches!(
            k.discover_mcp_tools("s"),
            Err(KernelError::McpServerDisabled(_))
        ));
    }

    #[test]
    fn mcp_discovery_maps_tools_into_descriptors() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        // A loopback mock MCP server: initialize → notif → tools/list.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let endpoint = format!("http://127.0.0.1:{}/mcp", addr.port());
        let responses = vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}"#,
            "{}",
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"search","description":"Find things."}]}}"#,
        ];
        thread::spawn(move || {
            for body in responses {
                let Ok((mut sock, _)) = listener.accept() else { break };
                // Drain the request headers + body before responding.
                let mut buf = [0u8; 4096];
                let mut data = Vec::new();
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
                let clen = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while data.len() - header_end < clen {
                    match sock.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                        Err(_) => break,
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
                let _: Result<(), _> = sock.flush();
            }
        });

        let mut k = KernelState::new();
        k.register_mcp_server("fs", &endpoint, "fs server", true, Some(2000))
            .unwrap();
        let tools = k.discover_mcp_tools("fs").expect("discovery ok");
        assert_eq!(tools.len(), 1);
        let t = &tools[0];
        assert_eq!(t.tool_name, "search");
        assert_eq!(t.plugin_id, "mcp:fs");
        assert_eq!(t.permission, "tool:mcp-fs:search");
        assert_eq!(t.source_kind, "Mcp");
        // An unclassified MCP tool is gated behind approval (Medium + Required),
        // never directly runnable, until the operator classifies it.
        assert_eq!(t.executable, ToolExecutability::NeedsApproval);
        assert_eq!(t.risk, relux_core::RiskLevel::Medium);

        // Classify it Low + auto-approve → it becomes directly runnable (`ready`).
        k.set_mcp_tool_classification(
            "fs",
            "search",
            relux_core::RiskLevel::Low,
            relux_core::ApprovalRequirement::Never,
        )
        .unwrap();
        // Re-discover requires another live probe; spin up a fresh mock for the
        // re-run is overkill — assert the executable mapping via a fresh descriptor
        // build is covered by the invocation tests below. Here just assert the
        // classification persisted on the config.
        assert!(k.mcp_server("fs").unwrap().tool_overrides.contains_key("search"));
    }

    // --- MCP tool invocation (loopback tools/call through the gates) -----------
    // `docs/mcp.md` "MCP tool invocation"; `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9.
    // These cover the FULL gate path: permission check, approval/risk gate, per-call
    // approval, persistent grant bypass, disabled-server + bad-name refusal, and the
    // no-raw-envelope contract — all through the SAME `invoke_tool` / approval / grant
    // surface a real plugin tool uses.

    /// Boot a mock loopback MCP server that answers `bodies` (raw JSON-RPC bodies)
    /// one per request, wrapping each in an HTTP 200. Returns its loopback endpoint.
    fn mock_mcp(bodies: Vec<String>) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let endpoint = format!("http://127.0.0.1:{}/mcp", addr.port());
        thread::spawn(move || {
            for body in bodies {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 4096];
                let mut data = Vec::new();
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
                let clen = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while data.len() - header_end < clen {
                    match sock.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                        Err(_) => break,
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
                let _: Result<(), _> = sock.flush();
            }
        });
        endpoint
    }

    /// The three response bodies for one successful tools/call: initialize ack,
    /// notifications/initialized ack, and a text-content result.
    fn mcp_call_bodies(text: &str) -> Vec<String> {
        vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}"#.to_string(),
            "{}".to_string(),
            format!(
                r#"{{"jsonrpc":"2.0","id":2,"result":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            ),
        ]
    }

    /// A primed kernel with an MCP server registered + Prime holding the MCP tool
    /// permission. `bodies` seed the mock server's responses.
    fn mcp_primed(bodies: Vec<String>) -> (KernelState, AgentId) {
        let (mut k, prime, _t, _r, _e) = primed_kernel();
        let endpoint = mock_mcp(bodies);
        k.register_mcp_server("fs", &endpoint, "fs server", true, Some(2000))
            .unwrap();
        k.grant_permission_to_agent(&prime, Permission::new("tool:mcp-fs:search").unwrap())
            .unwrap();
        (k, prime)
    }

    #[test]
    fn mcp_tool_unclassified_is_gated_even_with_permission() {
        // Default classification (Medium + Required) gates the direct invoke path
        // even though Prime holds the permission. No network call is made.
        let (mut k, prime) = mcp_primed(vec![]);
        let mcp = PluginId::new("mcp:fs");
        let err = k
            .invoke_tool(&prime, &mcp, "search", serde_json::json!({ "q": "x" }))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRequiresApproval { .. }), "got {err:?}");
        assert!(k.audit_log().iter().any(|e| e.action == "tool:mcp-fs:search"
            && e.result == AuditResult::Denied));
    }

    #[test]
    fn mcp_tool_denied_without_permission() {
        // Even a directly-runnable (Low + Never) MCP tool is refused for an agent
        // that does not hold its permission — the permission check runs first.
        let (mut k, _prime) = mcp_primed(vec![]);
        k.set_mcp_tool_classification(
            "fs",
            "search",
            relux_core::RiskLevel::Low,
            relux_core::ApprovalRequirement::Never,
        )
        .unwrap();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let weak = k
            .create_agent("weak", "Weak", "no perms", &adapter, &ns, None, vec![])
            .unwrap();
        let mcp = PluginId::new("mcp:fs");
        let err = k
            .invoke_tool(&weak, &mcp, "search", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");
    }

    #[test]
    fn mcp_tool_runs_through_gates_when_classified_runnable() {
        let (mut k, prime) = mcp_primed(mcp_call_bodies("hello from mcp"));
        k.set_mcp_tool_classification(
            "fs",
            "search",
            relux_core::RiskLevel::Low,
            relux_core::ApprovalRequirement::Never,
        )
        .unwrap();
        let mcp = PluginId::new("mcp:fs");
        let result = k
            .invoke_tool(&prime, &mcp, "search", serde_json::json!({ "q": "files" }))
            .expect("mcp invocation allowed");
        // The output is the SHAPED result, never the raw JSON-RPC envelope.
        assert_eq!(result.output, serde_json::json!({ "result": "hello from mcp" }));
        assert!(result.output.get("jsonrpc").is_none());
        assert_eq!(result.plugin_id, "mcp:fs");
        assert_eq!(result.tool_name, "search");
        assert_eq!(result.permission, "tool:mcp-fs:search");
        // Audited via the invoke path.
        assert!(k.audit_log().iter().any(|e| e.action == "tool:mcp-fs:search"
            && e.result == AuditResult::Success
            && e.metadata["via"] == "invoke"));
    }

    #[test]
    fn mcp_tool_persistent_grant_bypasses_approval() {
        // Default-gated tool + a standing allow-always grant → the direct invoke
        // runs (and records a grant:use), without lowering the tool's risk.
        let (mut k, prime) = mcp_primed(mcp_call_bodies("granted result"));
        let mcp = PluginId::new("mcp:fs");
        let grant = k
            .grant_persistent_tool_invocation("op", &prime, &mcp, "search")
            .expect("grant created");
        let result = k
            .invoke_tool(&prime, &mcp, "search", serde_json::json!({}))
            .expect("invocation via grant");
        assert_eq!(result.output, serde_json::json!({ "result": "granted result" }));
        assert!(k.audit_log().iter().any(|e| e.action == "grant:use"
            && e.metadata["grant"] == grant.id));
    }

    #[test]
    fn mcp_tool_per_call_approval_executes_once() {
        // Request a per-call approval (no network), approve it, then execute it —
        // the stored snapshot runs through the loopback MCP server exactly once.
        let (mut k, prime) = mcp_primed(mcp_call_bodies("approved call"));
        let mcp = PluginId::new("mcp:fs");
        let approval_id = k
            .request_tool_invocation_approval("op", &prime, &mcp, "search", serde_json::json!({ "q": "y" }))
            .expect("approval requested");
        k.resolve_approval(&approval_id, true, "op", None).unwrap();
        let result = k
            .execute_approved_tool_invocation(&approval_id, "op")
            .expect("execute approved");
        assert_eq!(result.output, serde_json::json!({ "result": "approved call" }));
        // A second execute is refused (consumed) — never runs twice on one approval.
        let err = k
            .execute_approved_tool_invocation(&approval_id, "op")
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolInvocationConsumed(_)), "got {err:?}");
    }

    #[test]
    fn mcp_tool_iserror_surfaces_as_runtime_failure() {
        let bodies = vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}"#.to_string(),
            "{}".to_string(),
            r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"nope"}],"isError":true}}"#.to_string(),
        ];
        let (mut k, prime) = mcp_primed(bodies);
        k.set_mcp_tool_classification(
            "fs",
            "search",
            relux_core::RiskLevel::Low,
            relux_core::ApprovalRequirement::Never,
        )
        .unwrap();
        let mcp = PluginId::new("mcp:fs");
        let err = k
            .invoke_tool(&prime, &mcp, "search", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::ToolRuntimeInvocation { .. }), "got {err:?}");
        assert!(k.audit_log().iter().any(|e| e.action == "tool:mcp-fs:search"
            && e.result == AuditResult::Failed));
    }

    #[test]
    fn mcp_disabled_server_refuses_invocation() {
        let (mut k, prime) = mcp_primed(vec![]);
        k.set_mcp_tool_classification(
            "fs",
            "search",
            relux_core::RiskLevel::Low,
            relux_core::ApprovalRequirement::Never,
        )
        .unwrap();
        k.set_mcp_server_enabled("fs", false).unwrap();
        let mcp = PluginId::new("mcp:fs");
        let err = k
            .invoke_tool(&prime, &mcp, "search", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::McpServerDisabled(_)), "got {err:?}");
    }

    #[test]
    fn mcp_unknown_server_and_bad_tool_name_refused() {
        let (mut k, prime) = mcp_primed(vec![]);
        // Unknown server id.
        let unknown = PluginId::new("mcp:ghost");
        let err = k
            .invoke_tool(&prime, &unknown, "search", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::UnknownMcpServer(_)), "got {err:?}");
        // A bad tool name is refused before any dial.
        let mcp = PluginId::new("mcp:fs");
        let err = k
            .invoke_tool(&prime, &mcp, "bad name", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidMcpToolName { .. }), "got {err:?}");
    }

    #[test]
    fn mcp_tool_classification_set_and_clear() {
        let (mut k, _prime) = mcp_primed(vec![]);
        let mcp = PluginId::new("mcp:fs");
        // Default → gated.
        assert!(k.tool_needs_approval(&mcp, "search"));
        // Classify Low + Never → no longer gated.
        k.set_mcp_tool_classification(
            "fs",
            "search",
            relux_core::RiskLevel::Low,
            relux_core::ApprovalRequirement::Never,
        )
        .unwrap();
        assert!(!k.tool_needs_approval(&mcp, "search"));
        // Clear → back to the gated default.
        k.clear_mcp_tool_classification("fs", "search").unwrap();
        assert!(k.tool_needs_approval(&mcp, "search"));
        // Classifying with a bad tool name is refused.
        assert!(matches!(
            k.set_mcp_tool_classification("fs", "bad name", relux_core::RiskLevel::Low, relux_core::ApprovalRequirement::Never),
            Err(KernelError::InvalidMcpToolName { .. })
        ));
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

    // --- Read-only context tool loop -------------------------------------------
    //
    // (`docs/prime-processing-audit.md` "Read-only tool loop"). The kernel takes a bounded,
    // read-only snapshot of live state; the pure executors read it and fabricate nothing.

    #[test]
    fn context_snapshot_feeds_the_read_only_tools_end_to_end() {
        use crate::prime_tools::{execute_context_tool, ToolCall};
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        let task_id = created.created_task.expect("a task was created").0;

        // The snapshot reflects the live board.
        let snap = k.context_snapshot(&ctx);
        assert!(snap.tasks.iter().any(|t| t.id == task_id));
        assert!(snap.agents.iter().any(|a| a.id == "researcher"));
        assert!(snap.agents.iter().any(|a| a.id == "prime"));

        // board_summary reads the real counts.
        let r = execute_context_tool(
            &snap,
            &ToolCall { tool: "board_summary".to_string(), args: Default::default() },
        );
        assert!(r.ok && r.detail.contains("tasks_total="));

        // get_task by the real id hits; an unknown id is an HONEST miss, never fabricated.
        let mut args = serde_json::Map::new();
        args.insert("task_id".to_string(), task_id.clone().into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_task".to_string(), args });
        assert!(r.ok && r.summary.contains(&task_id));

        let mut args = serde_json::Map::new();
        args.insert("task_id".to_string(), "task_9999".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_task".to_string(), args });
        assert!(!r.ok && r.detail.contains("does not exist"));

        // get_agent reads the roster.
        let mut args = serde_json::Map::new();
        args.insert("agent_id".to_string(), "researcher".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_agent".to_string(), args });
        assert!(r.ok && r.summary.contains("researcher"));

        // list_plugins reflects the live install index (the bundled prime adapter is protected).
        assert!(!snap.plugins.is_empty(), "the bundled prime adapter should be installed");
        let r = execute_context_tool(
            &snap,
            &ToolCall { tool: "list_plugins".to_string(), args: Default::default() },
        );
        assert!(r.ok && r.detail.contains("protected=true"));

        // list_approvals reads the (initially empty) approval set — an honest empty, never faked.
        let r = execute_context_tool(
            &snap,
            &ToolCall { tool: "list_approvals".to_string(), args: Default::default() },
        );
        assert!(r.ok);
        assert_eq!(snap.approvals.len(), 0);

        // get_run by an unknown id is an HONEST miss end-to-end, never fabricated.
        let mut args = serde_json::Map::new();
        args.insert("run_id".to_string(), "run_9999".into());
        let r = execute_context_tool(&snap, &ToolCall { tool: "get_run".to_string(), args });
        assert!(!r.ok && r.detail.contains("does not exist"));
    }

    #[test]
    fn unified_decision_tool_requests_execute_against_the_live_snapshot() {
        // The "read context on the unified decision" path end-to-end: the brain's ONE decision
        // envelope requests read-only context tools, the kernel parses + validates them against the
        // read-only allowlist, and the server-side deterministic executor runs them against the
        // live snapshot with no further brain call — the unified counterpart of the sidecar loop.
        use crate::prime_decision::parse_decision;
        use crate::prime_tools::execute_requested_reads;
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        let task_id = created.created_task.expect("a task was created").0;

        // A unified decision a status-question turn might produce: classify + request two reads,
        // one of them a smuggled mutating tool that must be rejected at parse time.
        let raw = format!(
            r#"{{"classification":{{"intent":"status_question","confidence":0.9}},
                 "tool_requests":[
                    {{"tool":"get_task","args":{{"task_id":"{task_id}"}}}},
                    {{"tool":"list_agents"}},
                    {{"tool":"delete_task","args":{{"task_id":"{task_id}"}}}}
                 ]}}"#
        );
        let decision = parse_decision(&raw).expect("a valid unified decision");
        // The mutating request was dropped; only the two read-only requests survive.
        assert_eq!(decision.context_requests.len(), 2);

        // Executed deterministically against the live snapshot — grounded, read-only, honest.
        let snap = k.context_snapshot(&ctx);
        let reads = execute_requested_reads(&snap, &decision.context_requests);
        assert_eq!(reads.len(), 2);
        let get_task = reads.iter().find(|r| r.tool == "get_task").unwrap();
        assert!(get_task.ok && get_task.summary.contains(&task_id));
        let list_agents = reads.iter().find(|r| r.tool == "list_agents").unwrap();
        assert!(list_agents.ok && list_agents.detail.contains("researcher"));
        // No mutation happened: the task is still on the board, unchanged.
        assert!(k.context_snapshot(&ctx).tasks.iter().any(|t| t.id == task_id));
    }

    // --- Multi-turn clarification memory ----------------------------------------
    //
    // (`docs/prime-processing-audit.md` "Multi-turn clarify memory"). A clarifying
    // question Prime asks for an actionable request is remembered so the NEXT message
    // can resolve it, while a fresh request / cancel / expiry never lets a stale
    // question silently steer a later message, and a risky action stays approval-gated.

    #[test]
    fn clarification_is_recorded_and_resolved_by_a_follow_up_answer() {
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        // Something to assign.
        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        let task_id = created.created_task.expect("a task was created");

        // Turn 1: an ambiguous assignment names the agent but not the task -> Clarify.
        let turn1 = k.prime_turn(&ctx, "assign this to researcher").unwrap();
        assert_eq!(turn1.disposition, PrimeDisposition::NeedsClarification);
        assert_eq!(turn1.intent, relux_core::PrimeIntent::AssignTask);
        let pending = k
            .pending_clarification_for(&ctx)
            .expect("a pending clarification was recorded");
        assert_eq!(pending.needs, "task id");
        assert_eq!(pending.intent, relux_core::PrimeIntent::AssignTask);

        // Turn 2: a bare answer (just the task id) resolves the ORIGINAL request, not a
        // generic chat reply. The combined message flows through the normal pipeline.
        let turn2 = k.prime_turn(&ctx, task_id.as_str()).unwrap();
        assert_eq!(turn2.disposition, PrimeDisposition::Executed);
        assert!(
            matches!(turn2.action, Some(relux_core::PrimeAction::AssignTask { .. })),
            "the follow-up answer continued the assignment: {:?}",
            turn2.action
        );
        assert!(
            k.pending_clarification_for(&ctx).is_none(),
            "the pending clarification is cleared once resolved"
        );
    }

    #[test]
    fn a_fuzzy_assignee_continuation_resolves_against_the_roster() {
        // The motivating dialogue end-to-end: "assign this to the researcher" (a fuzzy
        // reference, no task id) -> "which task?" -> "task_0001" continues the original
        // request, and the fuzzy "the researcher" resolves to the existing `researcher`
        // agent. Deterministic — no brain involved.
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        let task_id = created.created_task.expect("a task was created");

        // Turn 1: a fuzzy assignee with no task id -> Clarify (records the pending request).
        let turn1 = k.prime_turn(&ctx, "assign this to the researcher").unwrap();
        assert_eq!(turn1.disposition, PrimeDisposition::NeedsClarification);
        assert_eq!(turn1.intent, relux_core::PrimeIntent::AssignTask);

        // Turn 2: a bare task id continues, and "the researcher" resolves fuzzily.
        let turn2 = k.prime_turn(&ctx, task_id.as_str()).unwrap();
        assert_eq!(turn2.disposition, PrimeDisposition::Executed);
        match turn2.action {
            Some(relux_core::PrimeAction::AssignTask { task_id: t, agent_id }) => {
                assert_eq!(t, task_id.as_str());
                assert_eq!(agent_id, "researcher");
            }
            other => panic!("expected the fuzzy assignment to resolve, got {other:?}"),
        }
        assert_eq!(
            k.task(&task_id).unwrap().assigned_agent.as_ref(),
            Some(&AgentId::new("researcher"))
        );
    }

    #[test]
    fn a_run_start_clarification_is_resolved_by_a_task_id_follow_up() {
        // Two ready tasks -> "start it" asks which -> "task_xxxx" continues and starts that
        // one by id (the StartRun-by-id action wired this slice).
        let (mut k, ctx) = prime_chat_kernel();
        let a = k
            .prime_turn(&ctx, "create a task to run the tests")
            .unwrap()
            .created_task
            .expect("task a created");
        let b = k
            .prime_turn(&ctx, "create a task to build the docs")
            .unwrap()
            .created_task
            .expect("task b created");

        // Ambiguous run start -> Clarify, recorded as a resolvable pending (needs a task id).
        let turn1 = k.prime_turn(&ctx, "start it").unwrap();
        assert_eq!(turn1.disposition, PrimeDisposition::NeedsClarification);
        assert_eq!(turn1.intent, relux_core::PrimeIntent::RunStart);
        let pending = k
            .pending_clarification_for(&ctx)
            .expect("a run-start clarification was recorded");
        assert_eq!(pending.needs, "task id");

        // The bare task id continues and starts exactly that task.
        let turn2 = k.prime_turn(&ctx, b.as_str()).unwrap();
        assert_eq!(turn2.disposition, PrimeDisposition::Executed);
        match turn2.action {
            Some(relux_core::PrimeAction::StartRun { task_id }) => assert_eq!(task_id, b.as_str()),
            other => panic!("expected StartRun for the named id, got {other:?}"),
        }
        assert!(turn2.started_run.is_some());
        // The other ready task was left alone.
        assert_eq!(k.task(&a).unwrap().status, TaskStatus::Queued);
        assert!(k.pending_clarification_for(&ctx).is_none());
    }

    // --- Write-capable Prime tool surface --------------------------------------
    //
    // (`docs/prime-processing-audit.md` "A write-capable tool surface"). The brain may REQUEST a
    // governed write tool; the kernel maps it to an EXISTING action via the synthesized intent +
    // the validated slot, and routes it through the UNCHANGED `decide` → `prime_execute` / approval
    // path. The brain writes nothing directly; the fail-closed gate and every approval gate hold.

    #[test]
    fn write_tool_task_create_maps_to_the_existing_create_path() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "task.create",
            "args": {"title": "Fix the login redirect", "priority": 7}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Task(task) = &req.slot else {
            panic!("expected a task slot");
        };
        let (turn, source) = k
            .prime_turn_with_brain(
                &ctx,
                "create a task to fix the login bug",
                Some(&intent),
                BrainSlotProposals {
                    task: Some(task),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::TaskCreation);
        assert_eq!(source, crate::prime_intent::IntentSource::Brain);
        let created = turn.created_task.expect("the write tool created a task");
        // The validated slot title was applied through the existing create path.
        assert_eq!(k.task(&created).unwrap().title, "Fix the login redirect");
        assert!(turn.slots.is_some(), "the sharpened create carries a slot card");
    }

    #[test]
    fn write_tool_task_update_maps_to_the_existing_update_path() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let id = make_task(&mut k, &ctx);
        let tid = id.as_str().to_string();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "task.update",
            "args": {"task_id": tid, "priority": 8}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Update(update) = &req.slot else {
            panic!("expected an update slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "update a task",
                Some(&intent),
                BrainSlotProposals {
                    update: Some(update),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        assert_eq!(k.task(&id).unwrap().priority, 8);
    }

    #[test]
    fn write_tool_task_assign_maps_to_the_existing_assign_path() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        let id = make_task(&mut k, &ctx);
        let tid = id.as_str().to_string();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "task.assign",
            "args": {"task_id": tid, "agent_id": "researcher"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Assign(assign) = &req.slot else {
            panic!("expected an assign slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "assign a task",
                Some(&intent),
                BrainSlotProposals {
                    assign: Some(assign),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        assert_eq!(
            k.task(&id).unwrap().assigned_agent.as_ref(),
            Some(&AgentId::new("researcher"))
        );
    }

    #[test]
    fn write_tool_task_start_maps_to_the_existing_start_path() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        // Two ready tasks, so the deterministic plan can only ask which — the brain run slot
        // names exactly one (existence- AND readiness-checked against the live ready queue).
        let _a = k
            .prime_turn(&ctx, "create a task to run the tests")
            .unwrap()
            .created_task
            .unwrap();
        let b = k
            .prime_turn(&ctx, "create a task to build the docs")
            .unwrap()
            .created_task
            .unwrap();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "task.start",
            "args": {"task_id": b.as_str()}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::RunStart(run) = &req.slot else {
            panic!("expected a run-start slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "start a task",
                Some(&intent),
                BrainSlotProposals {
                    run: Some(run),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        match turn.action {
            Some(PrimeAction::StartRun { task_id }) => assert_eq!(task_id, b.as_str()),
            other => panic!("expected StartRun for the named id, got {other:?}"),
        }
        assert!(turn.started_run.is_some());
    }

    #[test]
    fn write_tool_task_start_rejects_an_unknown_or_unready_task() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let _a = k.prime_turn(&ctx, "create a task to run the tests").unwrap();
        let _b = k.prime_turn(&ctx, "create a task to build the docs").unwrap();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "task.start",
            "args": {"task_id": "task_9999"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::RunStart(run) = &req.slot else {
            panic!("expected a run-start slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "start a task",
                Some(&intent),
                BrainSlotProposals {
                    run: Some(run),
                    ..Default::default()
                },
            )
            .unwrap();
        // An unknown id never resolves — the deterministic clarify stands, nothing starts.
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.started_run.is_none());
    }

    #[test]
    fn write_tool_agent_create_maps_to_the_existing_agent_path() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "agent.create",
            "args": {"name": "Research Agent"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Agent(agent) = &req.slot else {
            panic!("expected an agent slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "create an agent",
                Some(&intent),
                BrainSlotProposals {
                    agent: Some(agent),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AgentCreation);
        // The validated name normalized to an id through the existing create path.
        assert_eq!(turn.created_agent.unwrap().as_str(), "research-agent");
    }

    #[test]
    fn write_tool_orchestration_create_maps_to_the_existing_orchestrate_path() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let before = k.orchestration_count();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.create",
            "args": {"goal": "research the market, build the landing page, and test it"}
        }))
        .unwrap();
        assert!(!req.gated, "orchestration.create is a safe Act, not approval-gated");
        let intent = req.intent_proposal();
        let WriteToolSlot::Orchestration(orch) = &req.slot else {
            panic!("expected an orchestration slot");
        };
        let (turn, source) = k
            .prime_turn_with_brain(
                &ctx,
                "orchestrate the launch",
                Some(&intent),
                BrainSlotProposals {
                    orchestration: Some(orch),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::Orchestration);
        assert_eq!(source, crate::prime_intent::IntentSource::Brain);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        // The validated goal flowed through the EXISTING deterministic orchestration creation
        // path, producing a real multi-brief orchestration.
        assert_eq!(
            k.orchestration_count(),
            before + 1,
            "the write tool created an orchestration through the existing path"
        );
        assert!(turn.created_task.is_some(), "the first brief is surfaced as the created task");
    }

    #[test]
    fn write_tool_orchestration_promotes_brain_named_steps() {
        // A single-clause message the deterministic planner would only CLARIFY is PROMOTED to a
        // real orchestration when the brain names the distinct steps — but only through the
        // deterministic planner (which still owns the briefs/agents/cap/DAG).
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let before = k.orchestration_count();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.create",
            "args": {"goal": "ship the launch",
                     "steps": ["research the market", "build the landing page", "test it"]}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Orchestration(orch) = &req.slot else {
            panic!("expected an orchestration slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "orchestrate the launch",
                Some(&intent),
                BrainSlotProposals {
                    orchestration: Some(orch),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        assert_eq!(k.orchestration_count(), before + 1);
    }

    #[test]
    fn write_tool_orchestration_drops_a_single_clause_goal() {
        // A goal that does not genuinely split is NOT orchestratable: the deterministic planner's
        // multi-agent constraint stands, so the deterministic clarify is kept and nothing is created.
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let before = k.orchestration_count();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.create",
            "args": {"goal": "summarize the README"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Orchestration(orch) = &req.slot else {
            panic!("expected an orchestration slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "orchestrate this",
                Some(&intent),
                BrainSlotProposals {
                    orchestration: Some(orch),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert_eq!(
            k.orchestration_count(),
            before,
            "a non-splittable goal must never fan out"
        );
    }

    #[test]
    fn casual_chat_never_triggers_orchestration() {
        // A guarded coordination QUESTION must never mint an orchestration via the write tool,
        // even with a valid multi-step goal slot. The deterministic classifier itself reads this
        // as `Orchestration` (so the intent gate's veto is a no-op), which is exactly why the
        // promotion is additionally gated on `!is_chat_guarded`: the guarded question keeps the
        // deterministic CLARIFY and creates nothing — only an explicit ask promotes.
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let before = k.orchestration_count();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.create",
            "args": {"goal": "research the options, build it, and test it"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Orchestration(orch) = &req.slot else {
            panic!("expected an orchestration slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "should we split this across a few agents?",
                Some(&intent),
                BrainSlotProposals {
                    orchestration: Some(orch),
                    ..Default::default()
                },
            )
            .unwrap();
        // SAFETY: the guarded question is answered with a clarification, not a fan-out of work.
        assert_ne!(
            turn.disposition,
            PrimeDisposition::Executed,
            "a guarded question must not execute an orchestration"
        );
        assert_eq!(
            k.orchestration_count(),
            before,
            "casual chat must never orchestrate via a write tool"
        );
    }

    #[test]
    fn write_tool_plugin_install_stays_approval_gated() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let before = k.installed_plugins().len();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "plugin.install",
            "args": {"plugin_id": "relux-tools-github"}
        }))
        .unwrap();
        assert!(req.gated, "plugin.install is approval-gated");
        let intent = req.intent_proposal();
        let WriteToolSlot::Plugin(plugin) = &req.slot else {
            panic!("expected a plugin slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "install the github plugin",
                Some(&intent),
                BrainSlotProposals {
                    plugin: Some(plugin),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        assert!(turn.approval.is_some(), "an install is proposed behind approval");
        // SAFETY: nothing was installed — a write tool cannot execute a protected install.
        assert_eq!(
            k.installed_plugins().len(),
            before,
            "a write tool must never install a plugin by itself"
        );
    }

    #[test]
    fn write_tool_permission_grant_stays_approval_gated() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let researcher = add_agent(&mut k, &ctx, "researcher");
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "permission.grant",
            "args": {"subject_kind": "agent", "subject_id": "researcher",
                     "permission": "tool:relux-tools-github:access"}
        }))
        .unwrap();
        assert!(req.gated, "permission.grant is approval-gated");
        let intent = req.intent_proposal();
        let WriteToolSlot::Permission(perm) = &req.slot else {
            panic!("expected a permission slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "grant a permission",
                Some(&intent),
                BrainSlotProposals {
                    permission: Some(perm),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        assert!(turn.approval.is_some());
        // SAFETY: nothing was granted — a write tool cannot execute a protected grant.
        assert!(
            k.agent(&researcher).unwrap().permissions.is_empty(),
            "a write tool must never grant a permission by itself"
        );
    }

    #[test]
    fn casual_chat_never_triggers_a_write_tool() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "task.create",
            "args": {"title": "Fix the login redirect"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Task(task) = &req.slot else {
            panic!("expected a task slot");
        };
        // Guarded chat (musing) + a sensitive write-tool intent → the fail-closed gate vetoes it.
        let (turn, source) = k
            .prime_turn_with_brain(
                &ctx,
                "we should maybe fix the login someday",
                Some(&intent),
                BrainSlotProposals {
                    task: Some(task),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::Brainstorming);
        assert_eq!(source, crate::prime_intent::IntentSource::Deterministic);
        assert!(
            turn.created_task.is_none(),
            "casual chat must never mint work via a write tool"
        );
    }

    #[test]
    fn write_tool_assign_fails_closed_on_an_unknown_task() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "task.assign",
            "args": {"task_id": "task_9999", "agent_id": "researcher"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Assign(assign) = &req.slot else {
            panic!("expected an assign slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "assign a task",
                Some(&intent),
                BrainSlotProposals {
                    assign: Some(assign),
                    ..Default::default()
                },
            )
            .unwrap();
        // An unknown task never validates — the deterministic clarify stands, nothing changes.
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.assign_slots.is_none());
    }

    // --- Post-execution (after-action) reply shaping ----------------------------
    //
    // (`docs/prime-processing-audit.md` "after-action narration"). The action has ALREADY run (or
    // been proposed) through the unchanged path; the brain only RE-WORDS the confirmation,
    // grounded in a sanitized result envelope and validated against it. These tests drive a REAL
    // executed/proposed turn through the kernel, then exercise the envelope + validator with
    // synthetic brain replies (no provider is ever called), asserting the wording is shaped while
    // durable state is unchanged by the shaping, and that a contradiction/failure falls back.

    #[test]
    fn after_action_shapes_a_real_create_but_changes_no_state() {
        use crate::prime_after_action::{
            after_action_kind, build_action_envelope, parse_after_action, reconcile_after_action,
            ActionResultKind,
        };
        let (mut k, ctx) = prime_chat_kernel();
        // A real deterministic create executes and leaves exactly one task.
        let turn = k
            .prime_turn(&ctx, "create a task to fix the login redirect")
            .unwrap();
        let created = turn.created_task.clone().expect("a task was created");
        let tasks_before = k.inspect_state().tasks_total;

        assert_eq!(after_action_kind(&turn), Some(ActionResultKind::Executed));
        let env = build_action_envelope(&turn, ActionResultKind::Executed);
        assert!(env.ids.contains(&created.as_str().to_string()));

        // A valid, grounded confirmation that references the real id is shaped and honored.
        let shaped = parse_after_action(
            &format!(
                r#"{{"text":"Done - I created {} to fix the login redirect.","confidence":0.9}}"#,
                created.as_str()
            ),
            &env,
        )
        .unwrap();
        assert!(reconcile_after_action(&env.grounded_reply, &shaped).is_some());

        // SAFETY: the shaping is pure — no new task appeared; the only task is the one the turn
        // already created.
        assert_eq!(k.inspect_state().tasks_total, tasks_before);
    }

    #[test]
    fn after_action_falls_back_when_the_brain_claims_unexecuted_work() {
        use crate::prime_after_action::{
            build_action_envelope, parse_after_action, ActionResultKind,
        };
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k
            .prime_turn(&ctx, "create a task to fix the login redirect")
            .unwrap();
        let env = build_action_envelope(&turn, ActionResultKind::Executed);
        // The create did NOT start a run; a reply that claims one is rejected → deterministic
        // wording stands (the server falls back to `shape_reply`).
        assert!(parse_after_action(
            r#"{"text":"Created the task and started the run.","confidence":0.95}"#,
            &env
        )
        .is_err());
    }

    #[test]
    fn after_action_proposal_must_not_say_installed_and_installs_nothing() {
        use crate::prime_after_action::{
            after_action_kind, build_action_envelope, parse_after_action, ActionResultKind,
        };
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        let (mut k, ctx) = prime_chat_kernel();
        let before = k.installed_plugins().len();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "plugin.install",
            "args": {"plugin_id": "relux-tools-github"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::Plugin(plugin) = &req.slot else {
            panic!("expected a plugin slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "install the github plugin",
                Some(&intent),
                BrainSlotProposals {
                    plugin: Some(plugin),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        // The after-action kind is Proposed, so a completion claim is rejected.
        assert_eq!(after_action_kind(&turn), Some(ActionResultKind::Proposed));
        let env = build_action_envelope(&turn, ActionResultKind::Proposed);
        assert!(parse_after_action(
            r#"{"text":"The plugin is now installed for you.","confidence":0.95}"#,
            &env
        )
        .is_err());
        // Proposal-language is accepted (it correctly says approval is needed).
        assert!(parse_after_action(
            r#"{"text":"I've proposed installing relux-tools-github; it needs your approval.","confidence":0.9}"#,
            &env
        )
        .is_ok());
        // SAFETY: nothing was installed by the shaping or the proposal.
        assert_eq!(k.installed_plugins().len(), before);
    }

    /// Create one task assigned to Prime and return its id, for the update tests.
    fn make_task(k: &mut KernelState, ctx: &PrimeContext) -> relux_core::TaskId {
        k.prime_turn(ctx, "create a task to summarize the README")
            .unwrap()
            .created_task
            .expect("a task was created")
    }

    fn update_proposal(confidence: f32) -> crate::prime_update_slots::BrainUpdateSlots {
        crate::prime_update_slots::BrainUpdateSlots {
            task_id: None,
            title: None,
            details: None,
            priority: None,
            status: None,
            assignee: None,
            confidence,
            rationale: String::new(),
        }
    }

    #[test]
    fn task_update_applies_each_supported_field() {
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        let id = make_task(&mut k, &ctx);
        let tid = id.as_str().to_string();

        // Title (rename).
        let t = k
            .prime_turn(&ctx, &format!("rename {tid} to Fix the login blank page"))
            .unwrap();
        assert_eq!(t.disposition, PrimeDisposition::Executed);
        assert_eq!(k.task(&id).unwrap().title, "Fix the login blank page");
        let card = t.update.expect("an update card");
        assert_eq!(card.task_id, tid);
        assert!(card.changes.iter().any(|c| c.field == "title"));
        // A deterministic update shows no brain chip.
        assert!(card.source.is_none());

        // Priority (clamped range).
        k.prime_turn(&ctx, &format!("set {tid} priority to 8")).unwrap();
        assert_eq!(k.task(&id).unwrap().priority, 8);

        // Details (folded into the task input).
        k.prime_turn(&ctx, &format!("set {tid} details to Users see a blank page after SSO"))
            .unwrap();
        assert_eq!(
            k.task(&id).unwrap().input["details"],
            serde_json::json!("Users see a blank page after SSO")
        );

        // Assignee (reassignment, validated against the roster).
        k.prime_turn(&ctx, &format!("reassign {tid} to researcher")).unwrap();
        assert_eq!(
            k.task(&id).unwrap().assigned_agent.as_ref(),
            Some(&AgentId::new("researcher"))
        );

        // Status (operator-settable: blocked).
        let t = k.prime_turn(&ctx, &format!("mark {tid} as blocked")).unwrap();
        assert_eq!(t.disposition, PrimeDisposition::Executed);
        assert_eq!(k.task(&id).unwrap().status, TaskStatus::Blocked);
    }

    #[test]
    fn task_update_fails_closed_on_unknown_task_and_agent() {
        let (mut k, ctx) = prime_chat_kernel();
        let id = make_task(&mut k, &ctx);

        // Unknown task id → honest reply (an answer, not a guessed edit), no change.
        let t = k.prime_turn(&ctx, "set task_9999 priority to 8").unwrap();
        assert_eq!(t.disposition, PrimeDisposition::Answered);
        assert!(t.reply.contains("does not exist"));
        assert!(t.update.is_none());

        // Unknown assignee → honest reply, the task is untouched.
        let t = k.prime_turn(&ctx, &format!("reassign {} to nobody-here", id.as_str())).unwrap();
        assert!(t.reply.contains("does not exist"), "got {:?}", t.reply);
        assert_eq!(
            k.task(&id).unwrap().assigned_agent.as_ref(),
            Some(&AgentId::new("prime"))
        );
        assert!(t.update.is_none());
    }

    #[test]
    fn task_update_refuses_completion_and_terminal_tasks() {
        let (mut k, ctx) = prime_chat_kernel();
        let id = make_task(&mut k, &ctx);

        // "mark it done" is honestly refused — Prime never fakes a completion.
        let t = k.prime_turn(&ctx, &format!("mark {} as done", id.as_str())).unwrap();
        assert!(t.reply.contains("run lifecycle"), "got {:?}", t.reply);
        assert_ne!(k.task(&id).unwrap().status, TaskStatus::Completed);

        // Cancel the task (a real terminal transition), then a later edit is refused.
        k.prime_turn(&ctx, &format!("cancel {}", id.as_str())).unwrap();
        assert_eq!(k.task(&id).unwrap().status, TaskStatus::Cancelled);
        let t = k.prime_turn(&ctx, &format!("set {} priority to 9", id.as_str())).unwrap();
        assert_eq!(t.disposition, PrimeDisposition::NeedsClarification);
        assert!(t.reply.contains("finished task"), "got {:?}", t.reply);
        // Priority was NOT changed on the terminal task.
        assert_eq!(k.task(&id).unwrap().priority, 5);
    }

    #[test]
    fn brain_update_slots_resolve_an_under_specified_update() {
        // "change task priority" names no task → the deterministic rail clarifies; a
        // validated brain proposal of {task_id, priority} promotes it to a real update.
        let (mut k, ctx) = prime_chat_kernel();
        let id = make_task(&mut k, &ctx);

        // "change task priority" names no task → the deterministic rail clarifies; the
        // validated brain proposal of {task_id, priority} promotes it to a real update.
        let mut proposal = update_proposal(0.9);
        proposal.task_id = Some(id.as_str().to_string());
        proposal.priority = Some(8);
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "change task priority",
                None,
                BrainSlotProposals {
                    update: Some(&proposal),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        assert_eq!(k.task(&id).unwrap().priority, 8);
        // The card carries the brain marker (the server later stamps the real label).
        let card = turn.update.expect("an update card");
        assert_eq!(card.source.as_deref(), Some("brain"));
    }

    #[test]
    fn brain_update_slots_fail_closed_on_an_unknown_task() {
        // A brain proposal naming a task that does not exist is rejected; the
        // deterministic clarify stands and nothing changes.
        let (mut k, ctx) = prime_chat_kernel();
        let id = make_task(&mut k, &ctx);
        let before = k.task(&id).unwrap().priority;

        let mut proposal = update_proposal(0.9);
        proposal.task_id = Some("task_9999".to_string());
        proposal.priority = Some(8);
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "change task priority",
                None,
                BrainSlotProposals {
                    update: Some(&proposal),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.update.is_none());
        assert_eq!(k.task(&id).unwrap().priority, before);
    }

    #[test]
    fn task_update_clarification_is_resolved_by_a_follow_up() {
        // "change task priority" → "which / what value?" → "task_0001 to 8" continues the
        // original request through the multi-turn memory (TaskUpdate is now resolvable).
        let (mut k, ctx) = prime_chat_kernel();
        let id = make_task(&mut k, &ctx);

        let turn1 = k.prime_turn(&ctx, "change task priority").unwrap();
        assert_eq!(turn1.disposition, PrimeDisposition::NeedsClarification);
        assert_eq!(turn1.intent, relux_core::PrimeIntent::TaskUpdate);
        assert!(k.pending_clarification_for(&ctx).is_some());

        let turn2 = k.prime_turn(&ctx, &format!("{} to 8", id.as_str())).unwrap();
        assert_eq!(turn2.disposition, PrimeDisposition::Executed);
        assert_eq!(k.task(&id).unwrap().priority, 8);
        assert!(k.pending_clarification_for(&ctx).is_none());
    }

    #[test]
    fn casual_chat_never_triggers_a_task_update() {
        // A musing that merely mentions a task must NOT edit it (the conversation guard /
        // anchored classify keep it a Brainstorming reply).
        let (mut k, ctx) = prime_chat_kernel();
        let id = make_task(&mut k, &ctx);
        let before = k.task(&id).unwrap().priority;
        let t = k
            .prime_turn(&ctx, "should we bump the priority on the readme work at some point?")
            .unwrap();
        assert_ne!(t.intent, relux_core::PrimeIntent::TaskUpdate);
        assert!(t.update.is_none());
        assert_eq!(k.task(&id).unwrap().priority, before);
    }

    #[test]
    fn conversation_turn_is_recorded_and_rendered_as_background_context() {
        // A completed turn is remembered, and the next turn's prompt context renders it as
        // clearly-labelled BACKGROUND (the continuity the brain reads, never an instruction).
        let (mut k, ctx) = prime_chat_kernel();
        assert_eq!(k.recent_conversation_context(&ctx), "");
        let mut turn = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        // The server records AFTER shaping; simulate the brain-shaped final reply the user saw.
        turn.reply = "Done — I created that task for you.".to_string();
        k.record_conversation_turn(
            &ctx,
            "create a task to summarize the README",
            &turn,
            &[relux_core::PrimeContextRead {
                tool: "list_tasks".to_string(),
                ok: true,
                summary: "2 task(s)".to_string(),
            }],
        );
        let context = k.recent_conversation_context(&ctx);
        assert!(context.contains("BACKGROUND CONTEXT"));
        assert!(context.contains("NOT a new instruction"));
        assert!(context.contains("create a task to summarize the README"));
        // The FINAL shaped reply (not an earlier draft) is what is remembered.
        assert!(context.contains("Prime: Done — I created that task for you."));
        // The created id is summarized into the rendered context.
        assert!(context.contains("created "));
        // The consulted read-only context (name + bounded summary) is rendered as background.
        assert!(context.contains("consulted: list_tasks: 2 task(s)"));
    }

    #[test]
    fn recorded_reply_is_the_final_shaped_reply_not_the_grounded_one() {
        // The server replaces `turn.reply` with the FINAL user-visible reply (a validated
        // brain-shaped / after-action wording) BEFORE recording, so the memory remembers what the
        // user actually saw — not the earlier deterministic draft. This pins that contract at the
        // record layer: whatever reply is on the turn at record time is stored and rendered.
        let (mut k, ctx) = prime_chat_kernel();
        let mut turn = k.prime_turn(&ctx, "what is going on?").unwrap();
        let grounded = turn.reply.clone();
        let shaped = format!("{grounded} (re-worded by the brain for you)");
        assert_ne!(grounded, shaped);
        turn.reply = shaped.clone();
        k.record_conversation_turn(&ctx, "what is going on?", &turn, &[]);
        let snap = k.snapshot();
        let entry = snap
            .conversation_histories
            .iter()
            .find(|e| e.key == KernelState::conversation_key(&ctx))
            .expect("a history entry");
        let rec = entry.turns.last().unwrap();
        assert_eq!(rec.reply, shaped);
        assert!(k.recent_conversation_context(&ctx).contains("re-worded by the brain"));
    }

    #[test]
    fn conversation_history_is_bounded_per_conversation() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "what is going on?").unwrap();
        for i in 0..(crate::prime_history::MAX_HISTORY_TURNS + 6) {
            k.record_conversation_turn(&ctx, &format!("message number {i}"), &turn, &[]);
        }
        let snap = k.snapshot();
        let entry = snap
            .conversation_histories
            .iter()
            .find(|e| e.key == KernelState::conversation_key(&ctx))
            .expect("a history entry");
        assert_eq!(entry.turns.len(), crate::prime_history::MAX_HISTORY_TURNS);
        // Only the most recent turns survive (the oldest were evicted from the front).
        assert_eq!(
            entry.turns.last().unwrap().user_message,
            format!("message number {}", crate::prime_history::MAX_HISTORY_TURNS + 5)
        );
    }

    #[test]
    fn conversation_history_redacts_secrets_and_stores_no_raw_envelope() {
        // A secret in the message / reply / read summary is masked before storage, and the tool's
        // raw result BODY is never persisted — only its name + bounded summary (no-raw-envelope).
        let (mut k, ctx) = prime_chat_kernel();
        let mut turn = k.prime_turn(&ctx, "what is going on?").unwrap();
        turn.reply = "Here is the answer with token=sk-ABCDEFGHIJKLMNOP0123456789".to_string();
        turn.tool_output = Some(serde_json::json!({ "leak": "sk-SHOULDNEVERPERSIST0001" }));
        k.record_conversation_turn(
            &ctx,
            "what is going on? api_key=sk-LEAKLEAKLEAKLEAK00012345",
            &turn,
            &[relux_core::PrimeContextRead {
                tool: "list_tasks".to_string(),
                ok: true,
                summary: "5 task(s) token=sk-SUMMARYLEAK00099988877".to_string(),
            }],
        );
        let snap = k.snapshot();
        let entry = snap
            .conversation_histories
            .iter()
            .find(|e| e.key == KernelState::conversation_key(&ctx))
            .expect("a history entry");
        let rec = entry.turns.last().unwrap();
        assert!(!rec.reply.contains("sk-ABCDEFGHIJKLMNOP0123456789"));
        assert!(!rec.user_message.contains("sk-LEAKLEAKLEAKLEAK00012345"));
        // The read is stored as name + its bounded summary, with any secret in the summary masked.
        assert_eq!(rec.tool_reads.len(), 1);
        assert!(rec.tool_reads[0].starts_with("list_tasks: 5 task(s)"));
        assert!(!rec.tool_reads[0].contains("sk-SUMMARYLEAK00099988877"));
        // The tool output body is NEVER persisted (no raw envelope / JSON).
        let serialized = serde_json::to_string(&snap.conversation_histories).unwrap();
        assert!(!serialized.contains("SHOULDNEVERPERSIST"));
    }

    #[test]
    fn conversation_history_survives_a_snapshot_round_trip() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "what is going on?").unwrap();
        k.record_conversation_turn(&ctx, "what is going on?", &turn, &[]);
        let before = k.recent_conversation_context(&ctx);
        assert!(!before.is_empty());
        let restored = KernelState::from_snapshot(k.snapshot());
        assert_eq!(restored.recent_conversation_context(&ctx), before);
    }

    #[test]
    fn conversation_summary_accumulates_on_eviction_and_renders_in_context() {
        // A turn that did durable work, then enough later turns to evict it from the recent ring:
        // the evicted turn is folded into a rolling summary so the long thread still remembers it,
        // even though it is no longer rendered verbatim.
        let (mut k, ctx) = prime_chat_kernel();
        let mut acted = k.prime_turn(&ctx, "what is going on?").unwrap();
        acted.reply = "Done — I created that.".to_string();
        acted.created_task = Some(relux_core::TaskId::new("task_0042"));
        k.record_conversation_turn(&ctx, "the very first request", &acted, &[]);
        // Push enough chat turns to evict the acting turn out of the front of the ring.
        let chat = k.prime_turn(&ctx, "what is going on?").unwrap();
        for i in 0..crate::prime_history::MAX_HISTORY_TURNS {
            k.record_conversation_turn(&ctx, &format!("later message {i}"), &chat, &[]);
        }
        // The acting turn is no longer in the verbatim ring...
        let snap = k.snapshot();
        let hist = snap
            .conversation_histories
            .iter()
            .find(|e| e.key == KernelState::conversation_key(&ctx))
            .expect("a history entry");
        assert!(!hist.turns.iter().any(|t| t.user_message == "the very first request"));
        // ...but it survives in the rolling summary, which renders inside the BACKGROUND block.
        let summary = snap
            .conversation_summaries
            .iter()
            .find(|e| e.key == KernelState::conversation_key(&ctx))
            .expect("a summary entry");
        assert!(summary.summary.highlights.iter().any(|h| h.contains("created task_0042")));
        assert_eq!(summary.summary.opened_with.as_deref(), Some("the very first request"));
        let context = k.recent_conversation_context(&ctx);
        assert!(context.contains("Summary of earlier turns"));
        assert!(context.contains("created task_0042"));
        assert!(context.contains("started with \"the very first request\""));
        // The summary sits ABOVE the verbatim recent turns in one block.
        assert!(context.find("Summary of earlier turns").unwrap() < context.find("User: later message").unwrap());
    }

    #[test]
    fn conversation_summary_survives_a_snapshot_round_trip() {
        let (mut k, ctx) = prime_chat_kernel();
        let mut acted = k.prime_turn(&ctx, "what is going on?").unwrap();
        acted.created_task = Some(relux_core::TaskId::new("task_0007"));
        k.record_conversation_turn(&ctx, "the opening ask", &acted, &[]);
        let chat = k.prime_turn(&ctx, "what is going on?").unwrap();
        for i in 0..crate::prime_history::MAX_HISTORY_TURNS {
            k.record_conversation_turn(&ctx, &format!("m{i}"), &chat, &[]);
        }
        let before = k.recent_conversation_context(&ctx);
        assert!(before.contains("Summary of earlier turns"));
        let restored = KernelState::from_snapshot(k.snapshot());
        assert_eq!(restored.recent_conversation_context(&ctx), before);
    }

    #[test]
    fn conversation_summary_redacts_secrets_and_stores_no_raw_envelope() {
        // A secret in the opening message is masked before it reaches the rolling summary anchor,
        // and no tool body is ever folded in (the summary carries only ids + counts).
        let (mut k, ctx) = prime_chat_kernel();
        let mut acted = k.prime_turn(&ctx, "what is going on?").unwrap();
        acted.created_task = Some(relux_core::TaskId::new("task_0001"));
        acted.tool_output = Some(serde_json::json!({ "leak": "sk-SUMMARYBODYLEAK00001" }));
        k.record_conversation_turn(&ctx, "open with token=sk-OPENINGLEAK000111222333", &acted, &[]);
        let chat = k.prime_turn(&ctx, "what is going on?").unwrap();
        for i in 0..crate::prime_history::MAX_HISTORY_TURNS {
            k.record_conversation_turn(&ctx, &format!("m{i}"), &chat, &[]);
        }
        let serialized = serde_json::to_string(&k.snapshot().conversation_summaries).unwrap();
        assert!(!serialized.contains("sk-OPENINGLEAK000111222333"));
        assert!(!serialized.contains("SUMMARYBODYLEAK"));
        assert!(serialized.contains("created task_0001"));
    }

    #[test]
    fn a_summary_full_of_actions_still_never_promotes_casual_chat_into_work() {
        // CURRENT-TURN SAFETY WINS OVER THE SUMMARY: even after many earlier turns created work
        // (all now compacted into the rolling summary), a casual musing on the next turn stays a
        // conversation and creates nothing. The summary is advisory prompt context only; the
        // deterministic classifier + fail-closed gate run on the CURRENT message alone.
        let (mut k, ctx) = prime_chat_kernel();
        let mut acted = k.prime_turn(&ctx, "what is going on?").unwrap();
        acted.created_task = Some(relux_core::TaskId::new("task_0001"));
        k.record_conversation_turn(&ctx, "make the first task", &acted, &[]);
        let chat = k.prime_turn(&ctx, "what is going on?").unwrap();
        for i in 0..crate::prime_history::MAX_HISTORY_TURNS {
            k.record_conversation_turn(&ctx, &format!("m{i}"), &chat, &[]);
        }
        assert!(k.recent_conversation_context(&ctx).contains("created task_0001"));
        let tasks_before = k.snapshot().tasks.len();

        let musing = k
            .prime_turn(&ctx, "i wonder if we should also clean up the docs someday")
            .unwrap();
        assert_eq!(musing.disposition, PrimeDisposition::Answered);
        assert_ne!(musing.intent, relux_core::PrimeIntent::TaskCreation);
        assert!(musing.created_task.is_none());
        assert_eq!(k.snapshot().tasks.len(), tasks_before);
    }

    #[test]
    fn clear_conversation_drops_history_and_any_pending_clarification() {
        let (mut k, ctx) = prime_chat_kernel();
        // Leave a pending clarification (an actionable, resolvable clarify).
        let t = k.prime_turn(&ctx, "assign this to the researcher").unwrap();
        assert_eq!(t.disposition, PrimeDisposition::NeedsClarification);
        assert!(k.pending_clarification_for(&ctx).is_some());
        // And some history.
        k.record_conversation_turn(&ctx, "assign this to the researcher", &t, &[]);
        assert!(!k.recent_conversation_context(&ctx).is_empty());

        assert!(k.clear_conversation(&ctx));
        assert!(k.pending_clarification_for(&ctx).is_none());
        assert_eq!(k.recent_conversation_context(&ctx), "");
        // A second clear has nothing left to drop.
        assert!(!k.clear_conversation(&ctx));
    }

    #[test]
    fn clear_conversation_drops_the_rolling_summary_too() {
        // Build a conversation long enough to evict turns into the rolling summary, then clear:
        // the summary must be dropped along with the ring (no advisory memory survives a reset).
        let (mut k, ctx) = prime_chat_kernel();
        let mut acted = k.prime_turn(&ctx, "what is going on?").unwrap();
        acted.created_task = Some(relux_core::TaskId::new("task_0001"));
        k.record_conversation_turn(&ctx, "the opening ask", &acted, &[]);
        let chat = k.prime_turn(&ctx, "what is going on?").unwrap();
        for i in 0..crate::prime_history::MAX_HISTORY_TURNS {
            k.record_conversation_turn(&ctx, &format!("m{i}"), &chat, &[]);
        }
        assert!(k.recent_conversation_context(&ctx).contains("Summary of earlier turns"));

        assert!(k.clear_conversation(&ctx));
        assert_eq!(k.recent_conversation_context(&ctx), "");
        assert!(
            !k.snapshot()
                .conversation_summaries
                .iter()
                .any(|e| e.key == KernelState::conversation_key(&ctx)),
            "the rolling summary is dropped on clear"
        );
        assert!(!k.clear_conversation(&ctx));
    }

    #[test]
    fn recorded_history_never_promotes_casual_chat_into_an_action() {
        // CURRENT-TURN SAFETY WINS OVER HISTORY: even after a prior turn created work, a casual
        // musing on the next turn stays a conversation and creates nothing. History is only
        // advisory prompt context; the deterministic classifier + fail-closed gate run on the
        // CURRENT message alone, so memory can never turn chat into an action.
        let (mut k, ctx) = prime_chat_kernel();
        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        assert_eq!(created.disposition, PrimeDisposition::Executed);
        k.record_conversation_turn(
            &ctx,
            "create a task to summarize the README",
            &created,
            &[],
        );
        let tasks_before = k.snapshot().tasks.len();

        let musing = k
            .prime_turn(&ctx, "i wonder if we should also clean up the docs someday")
            .unwrap();
        assert_eq!(musing.disposition, PrimeDisposition::Answered);
        assert_ne!(musing.intent, relux_core::PrimeIntent::TaskCreation);
        assert!(musing.created_task.is_none());
        assert_eq!(k.snapshot().tasks.len(), tasks_before);
    }

    #[test]
    fn an_explicit_cancellation_clears_the_pending_clarification() {
        let (mut k, ctx) = prime_chat_kernel();
        let turn1 = k.prime_turn(&ctx, "assign this to researcher").unwrap();
        assert_eq!(turn1.disposition, PrimeDisposition::NeedsClarification);
        assert!(k.pending_clarification_for(&ctx).is_some());

        // "never mind" drops the context and answers naturally — no action taken.
        let turn2 = k.prime_turn(&ctx, "never mind").unwrap();
        assert_eq!(turn2.disposition, PrimeDisposition::Answered);
        assert!(turn2.action.is_none());
        assert!(
            k.pending_clarification_for(&ctx).is_none(),
            "cancellation clears the pending clarification"
        );
    }

    #[test]
    fn an_unrelated_follow_up_supersedes_and_does_not_action_the_pending_request() {
        let (mut k, ctx) = prime_chat_kernel();
        let _ = k.prime_turn(&ctx, "assign this to researcher").unwrap();
        assert!(k.pending_clarification_for(&ctx).is_some());

        // A fresh question is not an answer to "which task?": it supersedes the pending
        // context and is handled on its own (a status report), never an assignment.
        let turn = k.prime_turn(&ctx, "what is the status?").unwrap();
        assert_ne!(turn.intent, relux_core::PrimeIntent::AssignTask);
        assert!(!matches!(
            turn.action,
            Some(relux_core::PrimeAction::AssignTask { .. })
        ));
        assert!(
            k.pending_clarification_for(&ctx).is_none(),
            "the stale pending context was dropped, not actioned"
        );
    }

    #[test]
    fn an_expired_clarification_is_ignored_and_does_not_continue() {
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "researcher");
        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        let task_id = created.created_task.unwrap();

        // Plant an ALREADY-expired pending clarification directly.
        let key = KernelState::conversation_key(&ctx);
        k.pending_clarifications.insert(
            key,
            relux_core::PendingClarification {
                original_message: "assign this to researcher".to_string(),
                intent: relux_core::PrimeIntent::AssignTask,
                needs: "task id".to_string(),
                question: "Which task should I assign?".to_string(),
                created_at_secs: 0,
                expires_at_secs: 0, // the clock has already advanced past 0
                source: "deterministic".to_string(),
            },
        );
        // It is not surfaced (expired).
        assert!(k.pending_clarification_for(&ctx).is_none());

        // And the bare answer is treated FRESH (no continuation), so it does NOT assign.
        let turn = k.prime_turn(&ctx, task_id.as_str()).unwrap();
        assert!(!matches!(
            turn.action,
            Some(relux_core::PrimeAction::AssignTask { .. })
        ));
    }

    #[test]
    fn a_risky_follow_up_still_requires_approval_through_the_memory_path() {
        let (mut k, ctx) = prime_chat_kernel();
        let _ = k.prime_turn(&ctx, "assign this to researcher").unwrap();
        assert!(k.pending_clarification_for(&ctx).is_some());
        let plugins_before = k.plugin_count();

        // A risky install request supersedes the pending clarification AND stays gated:
        // a brain/continuation can never execute a protected install by itself.
        let turn = k
            .prime_turn(&ctx, "install the relux-tools-github plugin")
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        assert!(turn.approval.is_some());
        assert_eq!(
            k.plugin_count(),
            plugins_before,
            "nothing was installed without an approval"
        );
        assert!(k.pending_clarification_for(&ctx).is_none());
    }

    #[test]
    fn pending_clarification_survives_a_snapshot_round_trip() {
        let (mut k, ctx) = prime_chat_kernel();
        let _ = k.prime_turn(&ctx, "assign this to researcher").unwrap();
        assert!(k.pending_clarification_for(&ctx).is_some());

        // The bounded record is persisted in the snapshot and restored on reload.
        let snapshot = k.snapshot();
        let restored = KernelState::from_snapshot(snapshot);
        let pending = restored
            .pending_clarification_for(&ctx)
            .expect("the pending clarification survived the snapshot round trip");
        assert_eq!(pending.intent, relux_core::PrimeIntent::AssignTask);
        assert_eq!(pending.needs, "task id");
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

    /// Add a second specialist agent so an assignee slot has a real target.
    fn add_agent(k: &mut KernelState, ctx: &PrimeContext, id: &str) -> AgentId {
        let adapter = PluginId::new("relux-adapter-local-prime");
        k.create_agent(
            id,
            id,
            "A specialist agent.",
            &adapter,
            &ctx.namespace,
            None,
            vec![],
        )
        .unwrap()
    }

    fn slots(title: &str, confidence: f32) -> crate::prime_slots::BrainTaskSlots {
        crate::prime_slots::BrainTaskSlots {
            title: title.to_string(),
            details: None,
            assignee: None,
            priority: None,
            confidence,
            rationale: String::new(),
        }
    }

    fn assign_slots(
        task_id: &str,
        agent_id: &str,
        confidence: f32,
    ) -> crate::prime_assign_slots::BrainAssignSlots {
        crate::prime_assign_slots::BrainAssignSlots {
            task_id: Some(task_id.to_string()),
            agent_id: Some(agent_id.to_string()),
            confidence,
            rationale: String::new(),
        }
    }

    #[test]
    fn brain_assign_slots_resolve_an_under_specified_assignment() {
        // The deterministic extractors cannot find a task id in "assign the readme task to
        // the helper" (no `task_` token), so the turn would clarify. A VALIDATED brain
        // proposal of {task_id, agent_id} — both existence-checked — promotes it to the
        // same safe AssignTask action, with honest provenance.
        let (mut k, ctx) = prime_chat_kernel();
        let helper = add_agent(&mut k, &ctx, "helper");
        let task_id = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap()
            .created_task
            .expect("a task was created");

        // Sanity: without the brain, this clarifies (no task id parsed).
        let (det, _src) = k
            .prime_turn_with_brain(
                &ctx,
                "assign the readme task to the helper",
                None,
                BrainSlotProposals::default(),
            )
            .unwrap();
        assert_eq!(det.disposition, PrimeDisposition::NeedsClarification);

        // With a validated brain proposal, the same message resolves to a real assignment.
        let p = assign_slots(task_id.as_str(), "helper", 0.9);
        let (turn, _src) = k
            .prime_turn_with_brain(
                &ctx,
                "assign the readme task to the helper",
                None,
                BrainSlotProposals {
                    assign: Some(&p),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        match turn.action {
            Some(relux_core::PrimeAction::AssignTask { task_id: t, agent_id }) => {
                assert_eq!(t, task_id.as_str());
                assert_eq!(agent_id, "helper");
            }
            other => panic!("expected a promoted AssignTask, got {other:?}"),
        }
        let prov = turn.assign_slots.expect("brain-resolved assignment surfaces provenance");
        assert_eq!(prov.task_id, task_id.as_str());
        assert_eq!(prov.agent_id, "helper");
        assert_eq!(k.task(&task_id).unwrap().assigned_agent.as_ref(), Some(&helper));
    }

    #[test]
    fn brain_assign_slots_fail_closed_on_an_unknown_id() {
        // A brain proposal naming a task/agent that does not exist can NOT invent an
        // assignment — the deterministic clarify stands.
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "helper");
        let _ = k.prime_turn(&ctx, "create a task to summarize the README").unwrap();

        let p = assign_slots("task_9999", "ghost", 0.9); // neither exists
        let (turn, _src) = k
            .prime_turn_with_brain(
                &ctx,
                "assign the readme task to the helper",
                None,
                BrainSlotProposals {
                    assign: Some(&p),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.action.is_none());
        assert!(turn.assign_slots.is_none());
    }

    #[test]
    fn continuation_slots_are_dropped_on_a_fresh_turn_and_vice_versa() {
        // The bundle is applied only to the message it was computed on: a bundle MARKED
        // continuation must NOT shape a fresh (non-continuation) turn.
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "helper");
        let task_id = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap()
            .created_task
            .unwrap();

        // A valid proposal, but marked as a continuation while this is a FRESH turn:
        // the safety gate drops it, so the turn clarifies instead of assigning.
        let p = assign_slots(task_id.as_str(), "helper", 0.9);
        let (turn, _src) = k
            .prime_turn_with_brain(
                &ctx,
                "assign the readme task to the helper",
                None,
                BrainSlotProposals {
                    assign: Some(&p),
                    continuation: true, // mismatched: this is not a continuation
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.action.is_none());
    }

    #[test]
    fn brain_slots_sharpen_a_created_task_title_details_priority_and_provenance() {
        // A validated brain slot proposal normalizes the title, folds details into
        // the task input, applies a clamped priority, and surfaces honest provenance
        // — while the create still flows through the deterministic execute path.
        let (mut k, ctx) = prime_chat_kernel();
        let mut s = slots("Fix the login redirect bug", 0.9);
        s.details = Some("Users land on a blank page after SSO.".to_string());
        s.priority = Some(8);

        let (turn, _src) = k
            .prime_turn_with_intent_and_slots(
                &ctx,
                "create a task to handle the login redirect mess",
                None,
                Some(&s),
            )
            .unwrap();

        assert_eq!(turn.intent, relux_core::PrimeIntent::TaskCreation);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        let task_id = turn.created_task.clone().expect("a task was created");
        let task = k.task(&task_id).unwrap();
        assert_eq!(task.title, "Fix the login redirect bug");
        assert_eq!(task.priority, 8);
        assert_eq!(
            task.input.get("details").and_then(|v| v.as_str()),
            Some("Users land on a blank page after SSO.")
        );
        // The created task carries the normalized title, not the run-on message.
        let provenance = turn.slots.expect("brain-assisted slots are surfaced");
        assert_eq!(provenance.title, "Fix the login redirect bug");
        assert_eq!(provenance.priority, Some(8));
        assert!(provenance.assignee.is_none());
        // The kernel leaves provenance source unset; the server stamps the label.
        assert!(provenance.source.is_none());
    }

    #[test]
    fn brain_slot_assignee_is_honored_only_when_the_agent_exists() {
        let (mut k, ctx) = prime_chat_kernel();
        let code_agent = add_agent(&mut k, &ctx, "code-agent");

        // A known assignee lands the task on that agent.
        let mut known = slots("Fix the login bug", 0.9);
        known.assignee = Some("code-agent".to_string());
        let (turn, _) = k
            .prime_turn_with_intent_and_slots(
                &ctx,
                "create a task to fix the login bug",
                None,
                Some(&known),
            )
            .unwrap();
        let task = k.task(&turn.created_task.unwrap()).unwrap();
        assert_eq!(task.assigned_agent.as_ref(), Some(&code_agent));
        assert_eq!(turn.slots.unwrap().assignee.as_deref(), Some("code-agent"));

        // An UNKNOWN assignee is dropped (fail closed); with nothing else
        // contributed it resolves to the deterministic slots — the task stays
        // assigned to Prime and no provenance chip is shown.
        let mut ghost = slots("summarize the readme", 0.9);
        ghost.assignee = Some("ghost-agent".to_string());
        let (turn2, _) = k
            .prime_turn_with_intent_and_slots(
                &ctx,
                "create a task to summarize the readme",
                None,
                Some(&ghost),
            )
            .unwrap();
        let task2 = k.task(&turn2.created_task.unwrap()).unwrap();
        assert_eq!(task2.assigned_agent.as_ref(), Some(&ctx.agent));
        assert!(turn2.slots.is_none());
    }

    #[test]
    fn no_slot_proposal_is_byte_for_byte_the_deterministic_create() {
        let (mut k, ctx) = prime_chat_kernel();
        let (turn, _) = k
            .prime_turn_with_intent_and_slots(
                &ctx,
                "create a task to summarize the readme",
                None,
                None,
            )
            .unwrap();
        let task = k.task(&turn.created_task.unwrap()).unwrap();
        assert_eq!(task.title, "summarize the readme");
        assert!(turn.slots.is_none(), "no brain assist means no slots field");
    }

    #[test]
    fn ideation_still_cannot_mint_a_task_even_with_a_slot_proposal() {
        // The intent gate keeps musing a conversation: the create path is never
        // reached, so a slot proposal can NEVER turn casual chat into work (§10.5).
        let (mut k, ctx) = prime_chat_kernel();
        let s = slots("Refactor the auth module", 0.99);
        let (turn, _) = k
            .prime_turn_with_intent_and_slots(
                &ctx,
                "we should refactor the auth module",
                None,
                Some(&s),
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::Brainstorming);
        assert!(turn.created_task.is_none());
        assert!(turn.slots.is_none());
        assert_eq!(k.task_count(), 0, "ideation must not create work");
    }

    #[test]
    fn create_and_run_sharpens_the_title_but_never_reassigns_the_run() {
        // The auto-run path may take a brain title, but the assignee is NOT applied:
        // the run stays on Prime (the only agent wired for the required grant), so
        // the brain can never reassign work it would immediately run.
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "code-agent");
        let mut s = slots("Ping the health endpoint", 0.9);
        s.assignee = Some("code-agent".to_string());

        let (turn, _) = k
            .prime_turn_with_intent_and_slots(
                &ctx,
                "create a task to ping the health endpoint and run it",
                None,
                Some(&s),
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::CreateAndRunTask);
        let task = k.task(&turn.created_task.clone().unwrap()).unwrap();
        assert_eq!(task.title, "Ping the health endpoint");
        // The run stayed on Prime despite the brain naming a different assignee.
        assert_eq!(task.assigned_agent.as_ref(), Some(&ctx.agent));
        assert!(turn.started_run.is_some());
        // Provenance reflects only what was applied: a sharpened title, no assignee.
        let provenance = turn.slots.expect("a sharpened title surfaces provenance");
        assert_eq!(provenance.title, "Ping the health endpoint");
        assert!(provenance.assignee.is_none());
    }

    // --- Unified brain decision (one envelope → intent + slots + wording) -----------------
    // These exercise the full unified path end-to-end EXCEPT the provider call: a realistic
    // synthetic envelope is parsed by `parse_decision`, decomposed into the SAME bundle the
    // server builds, and fed through the unchanged `prime_turn_with_brain` chokepoint. No test
    // calls a real provider.

    #[test]
    fn unified_decision_creates_a_task_with_title_and_details_in_one_envelope() {
        let (mut k, ctx) = prime_chat_kernel();
        let d = crate::prime_decision::parse_decision(
            r#"{"classification":{"intent":"task_creation","confidence":0.9},
                "task":{"title":"Fix the login redirect bug","details":"Blank page after SSO.","priority":8,"confidence":0.9}}"#,
        )
        .unwrap();
        // One envelope carried the intent AND the slots; the kernel reconciles both.
        let (turn, src) = k
            .prime_turn_with_brain(
                &ctx,
                "handle the login redirect mess",
                d.classification.as_ref(),
                BrainSlotProposals {
                    task: d.task.as_ref(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::TaskCreation);
        assert_eq!(src, crate::prime_intent::IntentSource::Brain);
        let task = k.task(&turn.created_task.unwrap()).unwrap();
        assert_eq!(task.title, "Fix the login redirect bug");
        assert_eq!(task.priority, 8);
        assert_eq!(
            task.input.get("details").and_then(|v| v.as_str()),
            Some("Blank page after SSO.")
        );
    }

    #[test]
    fn unified_decision_updates_a_task_by_id_in_one_envelope() {
        let (mut k, ctx) = prime_chat_kernel();
        let task_id = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap()
            .created_task
            .expect("a task was created");
        // The deterministic rail cannot resolve "bump the readme task priority" (no id), so it
        // clarifies; the unified decision carries the resolved id + field and promotes it.
        let d = crate::prime_decision::parse_decision(&format!(
            r#"{{"classification":{{"intent":"task_update","confidence":0.9}},
                "update":{{"task_id":"{}","priority":8,"confidence":0.9}}}}"#,
            task_id.as_str()
        ))
        .unwrap();
        let (turn, _src) = k
            .prime_turn_with_brain(
                &ctx,
                "bump the readme task priority",
                d.classification.as_ref(),
                BrainSlotProposals {
                    update: d.update.as_ref(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::TaskUpdate);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        assert_eq!(k.task(&task_id).unwrap().priority, 8);
    }

    #[test]
    fn unified_decision_supplies_validated_clarify_wording_in_one_envelope() {
        let (mut k, ctx) = prime_chat_kernel();
        // An under-specified assignment clarifies deterministically.
        let (turn, _src) = k
            .prime_turn_with_brain(&ctx, "assign this task", None, BrainSlotProposals::default())
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        let kind = crate::prime_clarify::clarify_polish_kind(&turn).expect("a clarify turn");
        // The wording the unified envelope carried is validated through the SAME clarify
        // chokepoint (exactly one question, no action claim) — no separate polish call.
        let d = crate::prime_decision::parse_decision(
            r#"{"wording":{"text":"Which task should I assign, and to whom?","confidence":0.9}}"#,
        )
        .unwrap();
        let polished = d
            .validated_wording(kind, &turn.reply)
            .expect("validated wording");
        assert!(polished.ends_with('?'));
        assert_ne!(polished, turn.reply);
    }

    #[test]
    fn unified_decision_ideation_still_creates_nothing() {
        // Even a maximally confident unified decision proposing task creation + slots cannot
        // mint work from guarded musing: the fail-closed intent gate keeps it a conversation.
        let (mut k, ctx) = prime_chat_kernel();
        let d = crate::prime_decision::parse_decision(
            r#"{"classification":{"intent":"task_creation","confidence":0.99},
                "task":{"title":"Refactor the auth module","confidence":0.99}}"#,
        )
        .unwrap();
        let (turn, src) = k
            .prime_turn_with_brain(
                &ctx,
                "we should refactor the auth module",
                d.classification.as_ref(),
                BrainSlotProposals {
                    task: d.task.as_ref(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::Brainstorming);
        assert!(turn.created_task.is_none());
        assert!(turn.slots.is_none());
        assert_eq!(src, crate::prime_intent::IntentSource::Deterministic);
        assert_eq!(k.task_count(), 0, "ideation must not create work");
    }

    fn agent_prop(name: &str, confidence: f32) -> crate::prime_agent_slots::BrainAgentSlots {
        crate::prime_agent_slots::BrainAgentSlots {
            name: name.to_string(),
            role: None,
            adapter: None,
            notes: None,
            persona: None,
            confidence,
            rationale: String::new(),
        }
    }

    #[test]
    fn brain_agent_slots_seed_a_starter_persona_on_the_created_agent() {
        // A validated persona is written to the created agent's durable `persona`
        // field (and surfaced in provenance), while the deterministic create still
        // gets None.
        let (mut k, ctx) = prime_chat_kernel();
        let mut s = agent_prop("CI Watcher", 0.9);
        s.role = Some("Watches CI".to_string());
        s.persona = Some("Methodical and concise; flags risks early.".to_string());

        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "create an agent to keep an eye on CI",
                None,
                BrainSlotProposals {
                    agent: Some(&s),
                    ..Default::default()
                },
            )
            .unwrap();

        let agent_id = turn.created_agent.clone().expect("an agent was created");
        let agent = k.agent(&agent_id).unwrap();
        assert_eq!(
            agent.persona.as_deref(),
            Some("Methodical and concise; flags risks early.")
        );
        let provenance = turn.agent_slots.expect("brain-assisted agent slots surfaced");
        assert_eq!(
            provenance.persona.as_deref(),
            Some("Methodical and concise; flags risks early.")
        );
    }

    #[test]
    fn clarify_polish_targets_only_nonactionful_clarify_and_brainstorm() {
        use crate::prime_clarify::{clarify_polish_kind, ClarifyKind};
        let (mut k, ctx) = prime_chat_kernel();

        // A musing turn → eligible for Brainstorm wording, and mints no task. Run first,
        // because a musing leaves no pending clarification behind (so the under-specified
        // update turn below is read fresh, not as a continuation of this one).
        let (muse, _) = k
            .prime_turn_with_brain(
                &ctx,
                "i was thinking we could redo the onboarding flow",
                None,
                BrainSlotProposals::default(),
            )
            .unwrap();
        assert_eq!(clarify_polish_kind(&muse), Some(ClarifyKind::Brainstorm));
        assert!(muse.created_task.is_none());

        // An under-specified TaskUpdate turn (no task named) clarifies → eligible for
        // Clarify wording, and it created nothing (action-free).
        let (update, _) = k
            .prime_turn_with_brain(&ctx, "reassign a task", None, BrainSlotProposals::default())
            .unwrap();
        assert_eq!(clarify_polish_kind(&update), Some(ClarifyKind::Clarify));
        assert!(update.created_task.is_none() && update.action.is_none());

        // A real create is ACTIONFUL → the wording path never touches it (the brain is
        // never near an action).
        let (create, _) = k
            .prime_turn_with_brain(
                &ctx,
                "create a task to summarize the README",
                None,
                BrainSlotProposals::default(),
            )
            .unwrap();
        assert!(create.created_task.is_some());
        assert_eq!(clarify_polish_kind(&create), None);
    }

    #[test]
    fn deterministic_agent_create_has_no_persona() {
        // With no brain slots, the created agent has no persona (the deterministic
        // fallback is unchanged).
        let (mut k, ctx) = prime_chat_kernel();
        let (turn, _) = k
            .prime_turn_with_brain(&ctx, "create an agent", None, BrainSlotProposals::default())
            .unwrap();
        let agent_id = turn.created_agent.clone().expect("an agent was created");
        assert!(k.agent(&agent_id).unwrap().persona.is_none());
    }

    #[test]
    fn brain_agent_slots_sharpen_a_created_agent_id_name_and_description() {
        // A validated agent-slot proposal normalizes the name into a clean id and
        // applies a real role/description — while the create still flows through the
        // deterministic execute path (the deterministic name would have been the
        // generic "new-agent" with "Agent created by Prime").
        let (mut k, ctx) = prime_chat_kernel();
        let mut s = agent_prop("CI Watcher", 0.9);
        s.role = Some("Watches CI and files a brief on failure".to_string());

        let (turn, _src) = k
            .prime_turn_with_brain(
                &ctx,
                "create an agent to keep an eye on CI",
                None,
                BrainSlotProposals {
                    agent: Some(&s),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(turn.intent, relux_core::PrimeIntent::AgentCreation);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        let agent_id = turn.created_agent.clone().expect("an agent was created");
        assert_eq!(agent_id.as_str(), "ci-watcher");
        let agent = k.agent(&agent_id).unwrap();
        assert_eq!(agent.name, "CI Watcher");
        assert_eq!(agent.description, "Watches CI and files a brief on failure");
        let provenance = turn.agent_slots.expect("brain-assisted agent slots surfaced");
        assert_eq!(provenance.id, "ci-watcher");
        assert_eq!(provenance.name, "CI Watcher");
        // The kernel leaves the provenance source unset; the server stamps the label.
        assert!(provenance.source.is_none());
    }

    #[test]
    fn brain_agent_slot_rejects_a_duplicate_id_and_keeps_the_deterministic_name() {
        // The brain proposes a name that normalizes to an existing agent id: the whole
        // proposal is rejected (fail closed) so a create can never be reshaped into a
        // collision, and the deterministic name stands.
        let (mut k, ctx) = prime_chat_kernel();
        add_agent(&mut k, &ctx, "research-agent");
        let mut dup = agent_prop("Research Agent", 0.9);
        dup.role = Some("Does research".to_string());

        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "create an agent",
                None,
                BrainSlotProposals {
                    agent: Some(&dup),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::AgentCreation);
        // Fell back to the deterministic "new-agent" name; no provenance chip.
        assert_eq!(turn.created_agent.unwrap().as_str(), "new-agent");
        assert!(turn.agent_slots.is_none());
    }

    #[test]
    fn no_agent_slot_proposal_is_byte_for_byte_the_deterministic_create() {
        let (mut k, ctx) = prime_chat_kernel();
        let (turn, _) = k
            .prime_turn_with_brain(&ctx, "create an agent", None, BrainSlotProposals::default())
            .unwrap();
        assert_eq!(turn.created_agent.unwrap().as_str(), "new-agent");
        assert!(turn.agent_slots.is_none());
    }

    fn permission_prop(
        subject_id: &str,
        permission: Option<&str>,
        confidence: f32,
    ) -> crate::prime_admin_slots::BrainPermissionSlots {
        crate::prime_admin_slots::BrainPermissionSlots {
            subject_kind: Some("agent".to_string()),
            subject_id: Some(subject_id.to_string()),
            permission: permission.map(|p| p.to_string()),
            confidence,
            rationale: String::new(),
        }
    }

    #[test]
    fn brain_permission_subject_sharpens_the_proposal_but_stays_approval_gated() {
        // The brain sharpens the subject of a risky grant — but the grant STAYS behind
        // a human approval: nothing is actually granted, only an approval is logged.
        let (mut k, ctx) = prime_chat_kernel();
        let code_agent = add_agent(&mut k, &ctx, "code-agent");
        // This message has no "agent" token, so the deterministic subject would be the
        // "(unspecified subject)" placeholder — the brain sharpens it to a real agent.
        let p = permission_prop("code-agent", Some("tool:relux-tools-github:access"), 0.9);

        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "give it access to github",
                None,
                BrainSlotProposals {
                    permission: Some(&p),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(turn.intent, relux_core::PrimeIntent::PermissionChange);
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        assert!(turn.approval.is_some(), "a grant is proposed behind approval");
        match turn.action.as_ref().expect("the proposed action is present") {
            PrimeAction::GrantPermission { subject_id, permission } => {
                assert_eq!(subject_id, "code-agent");
                assert_eq!(permission, "tool:relux-tools-github:access");
            }
            other => panic!("expected GrantPermission, got {other:?}"),
        }
        let admin = turn.admin_slots.expect("admin provenance surfaced");
        assert_eq!(admin.kind, "permission_grant");
        assert_eq!(admin.subject_id.as_deref(), Some("code-agent"));
        // SAFETY: the permission was NOT actually granted — the agent's permission set
        // is unchanged. Only a human approval can apply it.
        assert!(
            k.agent(&code_agent).unwrap().permissions.is_empty(),
            "a brain slot must never grant a permission by itself"
        );
    }

    #[test]
    fn brain_permission_subject_is_dropped_when_the_agent_does_not_exist() {
        let (mut k, ctx) = prime_chat_kernel();
        let unknown = permission_prop("ghost-agent", Some("tool:relux-tools-github:access"), 0.9);
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "give it access to github",
                None,
                BrainSlotProposals {
                    permission: Some(&unknown),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        // Unknown subject dropped → the deterministic placeholder subject stands and
        // no provenance is attached.
        match turn.action.as_ref().unwrap() {
            PrimeAction::GrantPermission { subject_id, .. } => {
                assert_eq!(subject_id, "(unspecified subject)");
            }
            other => panic!("expected GrantPermission, got {other:?}"),
        }
        assert!(turn.admin_slots.is_none());
    }

    #[test]
    fn brain_plugin_ref_sharpens_the_install_proposal_but_stays_approval_gated() {
        let (mut k, ctx) = prime_chat_kernel();
        let p = crate::prime_admin_slots::BrainPluginRef {
            plugin_id: "relux-tools-github".to_string(),
            confidence: 0.9,
            rationale: String::new(),
        };
        let plugins_before = k.plugin_count();

        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "install the plugin",
                None,
                BrainSlotProposals {
                    plugin: Some(&p),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(turn.intent, relux_core::PrimeIntent::PluginInstallation);
        assert_eq!(turn.disposition, PrimeDisposition::AwaitingApproval);
        assert!(turn.approval.is_some());
        match turn.action.as_ref().unwrap() {
            PrimeAction::InstallPlugin { plugin_id } => {
                assert_eq!(plugin_id, "relux-tools-github");
            }
            other => panic!("expected InstallPlugin, got {other:?}"),
        }
        let admin = turn.admin_slots.expect("admin provenance surfaced");
        assert_eq!(admin.kind, "plugin_install");
        assert_eq!(admin.plugin_id.as_deref(), Some("relux-tools-github"));
        // SAFETY: nothing was installed — only an approval was logged.
        assert_eq!(k.plugin_count(), plugins_before, "a brain slot must never install");
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

    /// The work-creation CTA labels that must NEVER appear on a casual / emotional /
    /// musing turn (Hermes-first suppression). A conversational turn may carry only
    /// contextual NON-action chips, never one of these.
    const WORK_CTA_LABELS: &[&str] = &[
        "Turn this into a task",
        "Plan this out",
        "Start the run",
        "Create these tasks",
    ];

    #[test]
    fn emotional_chat_gets_contextual_non_action_chips_not_work_ctas() {
        // Hermes-first suggestion policy (`docs/prime-processing-audit.md` "Hermes-first
        // general agent"): a vent, an insult, or frustration is `EmotionalSupport` — a
        // normal conversation. Prime answers and offers NO work CTA; instead it offers
        // CONTEXTUAL non-action chips ("Tell me what broke" / "Show me the last run")
        // that route ordinary read-only / conversational messages. Nothing is created.
        let (mut k, ctx) = prime_chat_kernel();
        let before = k.task_count();
        for msg in ["fuck you", "ugh this is so frustrating", "i give up", "i'm exhausted"] {
            let turn = k.prime_turn(&ctx, msg).unwrap();
            assert_eq!(
                turn.intent,
                relux_core::PrimeIntent::EmotionalSupport,
                "{msg:?} must classify as emotional support"
            );
            assert_eq!(turn.disposition, PrimeDisposition::Answered);
            for s in &turn.suggested_actions {
                assert!(
                    !WORK_CTA_LABELS.contains(&s.label.as_str()),
                    "{msg:?} must offer no work CTA, got {s:?}"
                );
            }
            assert!(
                turn.suggested_actions
                    .iter()
                    .any(|s| s.label == "Tell me what broke"),
                "{msg:?} offers a contextual non-action chip, got {:?}",
                turn.suggested_actions
            );
        }

        // Throwaway chitchat is `SmallTalk`: at most a single discovery chip, and
        // never a work CTA.
        for msg in ["lol", "haha nice", "thanks"] {
            let turn = k.prime_turn(&ctx, msg).unwrap();
            assert_eq!(
                turn.intent,
                relux_core::PrimeIntent::SmallTalk,
                "{msg:?} must classify as small talk"
            );
            for s in &turn.suggested_actions {
                assert!(
                    !WORK_CTA_LABELS.contains(&s.label.as_str()),
                    "{msg:?} must offer no work CTA, got {s:?}"
                );
            }
        }
        assert_eq!(k.task_count(), before, "emotional/casual chat must create no work");

        // A musing with no nameable work ("i was thinking about the weather")
        // classifies as Brainstorming deterministically, but carries no work verb, so
        // the gate still suppresses the CTAs — exercising the gate on the brainstorm path.
        let idle = k.prime_turn(&ctx, "i was thinking about the weather today").unwrap();
        assert_eq!(idle.intent, relux_core::PrimeIntent::Brainstorming);
        for s in &idle.suggested_actions {
            assert!(
                !WORK_CTA_LABELS.contains(&s.label.as_str()),
                "a contentless musing offers no work CTA, got {s:?}"
            );
        }
    }

    #[test]
    fn explicit_create_still_offers_the_start_cta() {
        // The Hermes-first suppression is surgical: an EXPLICIT work request still
        // yields its action and the "Start the run" CTA — only casual/emotional chat
        // loses the buttons.
        let (mut k, ctx) = prime_chat_kernel();
        let created = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        assert!(created.created_task.is_some(), "the explicit create still acts");
        assert!(
            created
                .suggested_actions
                .iter()
                .any(|s| s.label == "Start the run"),
            "an explicit create still offers Start the run: {:?}",
            created.suggested_actions
        );
    }

    #[test]
    fn plan_request_attaches_a_structured_action_free_proposal() {
        // §10 planning layer / §11.1: a plan request previews work as a STRUCTURED
        // proposal (so the dashboard renders a card, not parsed prose) and creates
        // NOTHING. The proposal mirrors the same decomposition the "Create these
        // tasks" commit is keyed on, and a normal turn never carries it.
        let (mut k, ctx) = prime_chat_kernel();
        let ns = ctx.namespace.clone();
        let adapter = PluginId::new("relux-adapter-local-prime");
        for id in ["research-agent", "code-agent", "doc-agent"] {
            k.create_agent(id, id, "specialist", &adapter, &ns, None, vec![])
                .unwrap();
        }

        let before = k.task_count();
        let turn = k
            .prime_turn(
                &ctx,
                "plan out research the options, build a prototype, and write the docs",
            )
            .unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::PlanRequest);
        // Action-free: the preview mints and runs nothing (§10.5, §17.1).
        assert_eq!(turn.disposition, PrimeDisposition::Answered);
        assert!(turn.created_task.is_none());
        assert_eq!(k.task_count(), before, "a plan preview must not create work");

        let proposal = turn.proposal.as_ref().expect("a plan request carries a proposal");
        assert!(proposal.multi_step, "a genuine split is a multi-step plan");
        assert!(
            proposal.steps.len() >= 2,
            "a multi-step plan lists its briefs: {:?}",
            proposal.steps
        );
        // The steps are positioned and grounded in roles/agents - never invented.
        assert_eq!(proposal.steps[0].index, 1);
        assert!(!proposal.steps[0].role.is_empty());
        assert!(!proposal.steps[0].agent.is_empty());
        assert!(!proposal.agents.is_empty(), "the plan names its assignees");
        // The proposal's goal is exactly what the commit suggestion re-wraps, so the
        // previewed and committed plans decompose from identical input.
        let commit = turn
            .suggested_actions
            .iter()
            .find(|s| s.label == "Create these tasks")
            .expect("a multi-step plan offers the explicit commit");
        assert_eq!(commit.message, format!("orchestrate {}", proposal.goal));
        assert!(!commit.send, "the commit pre-fills, never auto-sends");

        // A normal turn never carries the proposal (the wire stays unchanged for
        // existing clients).
        let greet = k.prime_turn(&ctx, "hey").unwrap();
        assert!(greet.proposal.is_none(), "a greeting carries no proposal");
        let made = k
            .prime_turn(&ctx, "create a task to summarize the README")
            .unwrap();
        assert!(
            made.proposal.is_none(),
            "task creation is not a plan preview - no proposal"
        );
    }

    #[test]
    fn plan_request_single_step_proposal_steers_to_one_task() {
        // A goal that does not genuinely split is steered to the one-task path: the
        // proposal is present (so the card can name the goal honestly) but flags
        // multi_step=false with no fanned-out steps (§10.5).
        let (mut k, ctx) = prime_chat_kernel();
        let turn = k.prime_turn(&ctx, "plan out summarizing the README").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::PlanRequest);
        let proposal = turn.proposal.as_ref().expect("a plan request carries a proposal");
        assert!(!proposal.multi_step, "a single piece of work is not a multi-step plan");
        assert!(proposal.steps.is_empty(), "a single-step plan fans out nothing");
        let one_task = turn
            .suggested_actions
            .iter()
            .find(|s| s.label == "Turn this into a task")
            .expect("a single-step plan offers the one-task route");
        assert!(!one_task.send, "the one-task route pre-fills, never auto-sends");
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

    #[test]
    fn revoke_permission_from_agent_works_audits_and_fails_closed() {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        let perm = Permission::new("tool:relux-tools-github:read").unwrap();

        // Revoking a permission the agent does not hold is an honest error, never a
        // silent no-op success.
        let err = k.revoke_permission_from_agent(&prime, &perm).unwrap_err();
        assert!(matches!(err, KernelError::PermissionNotGranted(..)), "got {err:?}");

        // Grant then revoke: the explicit list shrinks and the revoke is audited.
        k.grant_permission_to_agent(&prime, perm.clone()).unwrap();
        assert!(k.agent(&prime).unwrap().permissions.contains(&perm));
        k.revoke_permission_from_agent(&prime, &perm)
            .expect("should revoke a held permission");
        assert!(!k.agent(&prime).unwrap().permissions.contains(&perm));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "agent:revoke_permission" && e.result == AuditResult::Success));

        // Revoking a missing agent fails closed too.
        let ghost = AgentId::new("no-such-agent");
        let err = k.revoke_permission_from_agent(&ghost, &perm).unwrap_err();
        assert!(matches!(err, KernelError::UnknownAgent(_)), "got {err:?}");
    }

    #[test]
    fn scoped_wildcard_grant_authorizes_plugin_tools_and_revokes_exactly() {
        // A single scoped grant (`tool:<plugin>:*`) authorizes every concrete tool in
        // that plugin through the same `agent_holds_permission` chokepoint every
        // tool-invocation check routes through — without over-reaching to other plugins.
        let (mut k, prime, _task, _run, _echo) = primed_kernel();
        let scope = Permission::new("tool:relux-tools-github:*").unwrap();
        k.grant_permission_to_agent(&prime, scope.clone()).unwrap();

        let create_pr = Permission::new("tool:relux-tools-github:create_pr").unwrap();
        let merge_pr = Permission::new("tool:relux-tools-github:merge_pr").unwrap();
        let other_plugin = Permission::new("tool:relux-tools-gitlab:create_pr").unwrap();
        assert!(k.agent_holds_permission(&prime, &create_pr));
        assert!(k.agent_holds_permission(&prime, &merge_pr));
        assert!(
            !k.agent_holds_permission(&prime, &other_plugin),
            "a scoped grant must not authorize a different plugin"
        );

        // Revoke removes EXACTLY the stored scoped row (matches_exact bookkeeping); it
        // does not pattern-expand into the concrete tool perms, and once gone the
        // wildcard no longer authorizes anything.
        let err = k
            .revoke_permission_from_agent(&prime, &create_pr)
            .unwrap_err();
        assert!(
            matches!(err, KernelError::PermissionNotGranted(..)),
            "revoking a concrete tool the agent only holds via a scope must fail closed: {err:?}"
        );
        k.revoke_permission_from_agent(&prime, &scope)
            .expect("revoking the exact scoped grant succeeds");
        assert!(!k.agent(&prime).unwrap().permissions.contains(&scope));
        assert!(
            !k.agent_holds_permission(&prime, &create_pr),
            "after revoking the scope the agent holds nothing"
        );
    }

    #[test]
    fn manager_subtree_grant_enforces_branch_liveness_and_audits() {
        // Topology: director <- lead <- ic ; peer reports to director (lead's sibling);
        // outsider is top-level + unrelated. lead holds an `agent:lead:subtree:grant_permission`
        // scope over its own Branch.
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let mk = |k: &mut KernelState, id: &str, lead: Option<AgentId>| {
            k.create_agent_with_skills(
                id, id, "", &adapter, &ns, None, vec![], vec![], lead,
            )
            .unwrap()
        };
        let director = mk(&mut k, "director", None);
        let lead = mk(&mut k, "lead", Some(director.clone()));
        let ic = mk(&mut k, "ic", Some(lead.clone()));
        let peer = mk(&mut k, "peer", Some(director.clone()));
        let outsider = mk(&mut k, "outsider", None);

        let scope = Permission::new("agent:lead:subtree:grant_permission").unwrap();
        k.grant_permission_to_agent(&lead, scope).unwrap();
        let perm = Permission::new("tool:relux-tools-github:create_pr").unwrap();

        // (1) A real subordinate (direct child) — authorized; the target now holds it.
        k.manager_grant_permission_to_subordinate(&lead, &ic, perm.clone())
            .expect("lead may grant to its subordinate ic");
        assert!(k.agent_holds_permission(&ic, &perm));
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "agent:grant_permission" && e.result == AuditResult::Success));

        // (2) Sibling (peer) — denied; nothing granted; the denial is audited.
        let err = k
            .manager_grant_permission_to_subordinate(&lead, &peer, perm.clone())
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");
        assert!(!k.agent_holds_permission(&peer, &perm));

        // (3) Manager / ancestor (director) — denied (a node is not in its own subtree, and
        // a child is not above its lead).
        let err = k
            .manager_grant_permission_to_subordinate(&lead, &director, perm.clone())
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");

        // (4) Self — denied (proper-descendant semantics).
        let err = k
            .manager_grant_permission_to_subordinate(&lead, &lead, perm.clone())
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");

        // (5) Unrelated operative — denied.
        let err = k
            .manager_grant_permission_to_subordinate(&lead, &outsider, perm.clone())
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");

        // (6) Liveness (documented disabled-manager decision): pause lead — it now wields
        // NO subtree authority, even over a genuine subordinate.
        k.update_agent(&lead, None, None, None, None, Some(AgentStatus::Paused))
            .unwrap();
        let err = k
            .manager_grant_permission_to_subordinate(&lead, &ic, perm.clone())
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "paused manager denied: {err:?}");

        // (7) Missing scope: a manager WITHOUT the subtree grant cannot reach its subordinate.
        // director has a real subordinate (lead) but holds no subtree scope.
        let err = k
            .manager_grant_permission_to_subordinate(&director, &lead, perm)
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "no scope => denied: {err:?}");

        // A denial was audited for the manager-grant path.
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "agent:manager_grant_permission" && e.result == AuditResult::Denied));
    }

    #[test]
    fn agent_authenticated_manager_grant_enforces_authority_and_records_token_provenance() {
        // The per-agent-authenticated path (§19 follow-up): a manager that authenticated
        // its OWN request via a token drives the grant, with no operator in the loop. The
        // authority check is identical to the operator-assisted path; only the audit
        // provenance differs (token-actor vs operator).
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let mk = |k: &mut KernelState, id: &str, lead: Option<AgentId>| {
            k.create_agent_with_skills(id, id, "", &adapter, &ns, None, vec![], vec![], lead)
                .unwrap()
        };
        let lead = mk(&mut k, "lead", None);
        let ic = mk(&mut k, "ic", Some(lead.clone()));
        let outsider = mk(&mut k, "outsider", None);

        let scope = Permission::new("agent:lead:subtree:grant_permission").unwrap();
        k.grant_permission_to_agent(&lead, scope).unwrap();
        let perm = Permission::new("tool:relux-tools-github:create_pr").unwrap();

        // Authorized: the token-authenticated lead grants to its own subordinate.
        k.manager_grant_permission_to_subordinate_as_agent("agt_abc123", &lead, &ic, perm.clone())
            .expect("token-authenticated lead may grant to its subordinate");
        assert!(k.agent_holds_permission(&ic, &perm));
        // The inner agent-actor audit AND the token-provenance audit are both present.
        assert!(k.audit_log().iter().any(|e| {
            e.action == "agent:grant_permission" && e.result == AuditResult::Success
        }));
        let prov = k
            .audit_log()
            .iter()
            .find(|e| {
                e.action == "agent:token_authenticated_manager_grant"
                    && e.result == AuditResult::Success
            })
            .expect("a token-provenance audit row");
        assert_eq!(prov.actor_type, "agent");
        assert_eq!(prov.actor_id, "lead");
        // The PUBLIC token handle is recorded; the raw token never reaches the kernel.
        assert_eq!(prov.metadata["token_ref"], "agt_abc123");
        assert_eq!(prov.metadata["auth_source"], "agent_token");

        // Denied: the same actor cannot reach an unrelated operative, and the denial is
        // audited as a token-authenticated attempt (Denied).
        let err = k
            .manager_grant_permission_to_subordinate_as_agent("agt_abc123", &lead, &outsider, perm)
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "got {err:?}");
        assert!(k.audit_log().iter().any(|e| {
            e.action == "agent:token_authenticated_manager_grant" && e.result == AuditResult::Denied
        }));
    }

    #[test]
    fn agent_authenticated_manager_assign_task_enforces_authority_and_assignability() {
        // The second subtree action (`assign_task`): a manager that authenticated its OWN
        // request via a token assigns an existing task to one of its Branch subordinates.
        // Authority is the SAME `manager_subtree_authorizes` gate as the grant path; here
        // the action is `assign_task` and an extra assignability (non-terminal) rule applies.
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let mk = |k: &mut KernelState, id: &str, lead: Option<AgentId>| {
            k.create_agent_with_skills(id, id, "", &adapter, &ns, None, vec![], vec![], lead)
                .unwrap()
        };
        // director <- lead <- ic ; peer reports to director (lead's sibling); outsider unrelated.
        let director = mk(&mut k, "director", None);
        let lead = mk(&mut k, "lead", Some(director.clone()));
        let ic = mk(&mut k, "ic", Some(lead.clone()));
        let peer = mk(&mut k, "peer", Some(director.clone()));
        let outsider = mk(&mut k, "outsider", None);

        // lead is scoped ONLY for assign_task over its own Branch.
        let scope = Permission::new("agent:lead:subtree:assign_task").unwrap();
        k.grant_permission_to_agent(&lead, scope).unwrap();

        let mk_task = |k: &mut KernelState| {
            k.create_task("ship it", serde_json::Value::Null, "operator", &ns, vec![])
        };

        // (1) Authorized: the token-authenticated lead assigns a live task to subordinate ic.
        let t1 = mk_task(&mut k);
        k.manager_assign_task_to_subordinate_as_agent("agt_x1", &lead, &ic, &t1)
            .expect("lead may assign to its subordinate ic");
        let assigned = k.task(&t1).unwrap();
        assert_eq!(assigned.assigned_agent.as_ref(), Some(&ic));
        assert_eq!(assigned.status, TaskStatus::Queued);
        // Both the inner `task:assign` and the token-provenance row are recorded.
        assert!(k.audit_log().iter().any(|e| {
            e.action == "task:assign" && e.result == AuditResult::Success
        }));
        let prov = k
            .audit_log()
            .iter()
            .find(|e| {
                e.action == "agent:token_authenticated_manager_assign_task"
                    && e.result == AuditResult::Success
            })
            .expect("a token-provenance audit row");
        assert_eq!(prov.actor_type, "agent");
        assert_eq!(prov.actor_id, "lead");
        assert_eq!(prov.metadata["token_ref"], "agt_x1");
        assert_eq!(prov.metadata["auth_source"], "agent_token");
        assert_eq!(prov.metadata["target"], "ic");

        // (2) Sibling / ancestor / self / unrelated targets are all denied (403-equivalent).
        let t2 = mk_task(&mut k);
        for bad in [&peer, &director, &lead, &outsider] {
            let err = k
                .manager_assign_task_to_subordinate_as_agent("agt_x1", &lead, bad, &t2)
                .unwrap_err();
            assert!(
                matches!(err, KernelError::PermissionDenied { .. }),
                "target {bad:?} must be denied: {err:?}"
            );
        }
        // t2 was never assigned by a denied attempt.
        assert!(k.task(&t2).unwrap().assigned_agent.is_none());

        // (3) A manager WITHOUT an assign_task scope is denied even over a real subordinate
        // (director has subordinate lead but holds no subtree scope).
        let err = k
            .manager_assign_task_to_subordinate_as_agent("agt_x1", &director, &lead, &t2)
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "no scope => denied: {err:?}");

        // (4) Missing task → UnknownTask (after authority passes), assigns nothing.
        let ghost = TaskId::new("task_does_not_exist");
        let err = k
            .manager_assign_task_to_subordinate_as_agent("agt_x1", &lead, &ic, &ghost)
            .unwrap_err();
        assert!(matches!(err, KernelError::UnknownTask(_)), "missing task: {err:?}");

        // (5) Terminal task → TaskNotAssignable; a completed task cannot be reassigned.
        let t3 = mk_task(&mut k);
        k.complete_task(&t3).unwrap();
        let err = k
            .manager_assign_task_to_subordinate_as_agent("agt_x1", &lead, &ic, &t3)
            .unwrap_err();
        assert!(matches!(err, KernelError::TaskNotAssignable { .. }), "terminal task: {err:?}");
        // The completed task is untouched.
        assert_eq!(k.task(&t3).unwrap().status, TaskStatus::Completed);

        // (6) Liveness: a paused manager wields no subtree authority, even over a subordinate.
        k.update_agent(&lead, None, None, None, None, Some(AgentStatus::Paused))
            .unwrap();
        let t4 = mk_task(&mut k);
        let err = k
            .manager_assign_task_to_subordinate_as_agent("agt_x1", &lead, &ic, &t4)
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "paused manager denied: {err:?}");

        // A token-authenticated denial was audited.
        assert!(k.audit_log().iter().any(|e| {
            e.action == "agent:token_authenticated_manager_assign_task"
                && e.result == AuditResult::Denied
        }));
    }

    #[test]
    fn agent_authenticated_manager_revoke_permission_enforces_authority_and_holding() {
        // The third subtree action (`revoke_permission`): a manager that authenticated its
        // OWN request via a token revokes an explicit permission from one of its Branch
        // subordinates. Authority is the SAME `manager_subtree_authorizes` gate as the
        // grant/assign paths; here the action is `revoke_permission` and the inner revoke
        // removes EXACTLY the stored grant (honest PermissionNotGranted when not held).
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let mk = |k: &mut KernelState, id: &str, lead: Option<AgentId>| {
            k.create_agent_with_skills(id, id, "", &adapter, &ns, None, vec![], vec![], lead)
                .unwrap()
        };
        // director <- lead <- ic ; peer reports to director (lead's sibling); outsider unrelated.
        let director = mk(&mut k, "director", None);
        let lead = mk(&mut k, "lead", Some(director.clone()));
        let ic = mk(&mut k, "ic", Some(lead.clone()));
        let peer = mk(&mut k, "peer", Some(director.clone()));
        let outsider = mk(&mut k, "outsider", None);

        // lead is scoped ONLY for revoke_permission over its own Branch.
        let scope = Permission::new("agent:lead:subtree:revoke_permission").unwrap();
        k.grant_permission_to_agent(&lead, scope).unwrap();
        let perm = Permission::new("tool:relux-tools-github:create_pr").unwrap();
        // ic and peer both hold the concrete permission (granted via the operator path).
        k.grant_permission_to_agent(&ic, perm.clone()).unwrap();
        k.grant_permission_to_agent(&peer, perm.clone()).unwrap();

        // (1) Authorized: the token-authenticated lead revokes from its own subordinate ic.
        k.manager_revoke_permission_from_subordinate_as_agent("agt_r1", &lead, &ic, &perm)
            .expect("lead may revoke from its subordinate ic");
        assert!(
            !k.agent_holds_permission(&ic, &perm),
            "ic should no longer hold the revoked permission"
        );
        // Both the inner `agent:revoke_permission` and the token-provenance row are recorded.
        assert!(k.audit_log().iter().any(|e| {
            e.action == "agent:revoke_permission" && e.result == AuditResult::Success
        }));
        let prov = k
            .audit_log()
            .iter()
            .find(|e| {
                e.action == "agent:token_authenticated_manager_revoke_permission"
                    && e.result == AuditResult::Success
            })
            .expect("a token-provenance audit row");
        assert_eq!(prov.actor_type, "agent");
        assert_eq!(prov.actor_id, "lead");
        assert_eq!(prov.metadata["token_ref"], "agt_r1");
        assert_eq!(prov.metadata["auth_source"], "agent_token");
        assert_eq!(prov.metadata["permission"], "tool:relux-tools-github:create_pr");

        // (2) Sibling / ancestor / self / unrelated targets are all denied — authority is
        // checked FIRST, so peer keeps the permission it actually holds.
        for bad in [&peer, &director, &lead, &outsider] {
            let err = k
                .manager_revoke_permission_from_subordinate_as_agent("agt_r1", &lead, bad, &perm)
                .unwrap_err();
            assert!(
                matches!(err, KernelError::PermissionDenied { .. }),
                "target {bad:?} must be denied: {err:?}"
            );
        }
        assert!(
            k.agent_holds_permission(&peer, &perm),
            "a denied revoke must not touch peer's permission"
        );

        // (3) A manager WITHOUT a revoke_permission scope is denied even over a real
        // subordinate (director has subordinate lead but holds no subtree scope).
        let err = k
            .manager_revoke_permission_from_subordinate_as_agent("agt_r1", &director, &lead, &perm)
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "no scope => denied: {err:?}");

        // (4) Permission NOT held: after authority passes, revoking a permission ic does not
        // hold is the honest PermissionNotGranted (the inner revoke's fail-closed contract),
        // never a silent success.
        let err = k
            .manager_revoke_permission_from_subordinate_as_agent("agt_r1", &lead, &ic, &perm)
            .unwrap_err();
        assert!(
            matches!(err, KernelError::PermissionNotGranted(..)),
            "revoking an unheld permission must fail closed: {err:?}"
        );

        // (5) No pattern expansion: a `tool:<plugin>:*` scope held by ic is only revoked by
        // revoking that exact scope row, not a concrete tool inside it.
        let wildcard = Permission::new("tool:relux-tools-echo:*").unwrap();
        let concrete = Permission::new("tool:relux-tools-echo:say").unwrap();
        k.grant_permission_to_agent(&ic, wildcard.clone()).unwrap();
        let err = k
            .manager_revoke_permission_from_subordinate_as_agent("agt_r1", &lead, &ic, &concrete)
            .unwrap_err();
        assert!(
            matches!(err, KernelError::PermissionNotGranted(..)),
            "revoking a concrete tool ic only holds via a scope must fail closed: {err:?}"
        );
        assert!(
            k.agent_holds_permission(&ic, &concrete),
            "the scope still authorizes the concrete tool after a failed exact revoke"
        );
        k.manager_revoke_permission_from_subordinate_as_agent("agt_r1", &lead, &ic, &wildcard)
            .expect("revoking the exact scope row succeeds");
        assert!(!k.agent_holds_permission(&ic, &concrete));

        // (6) Liveness: a paused manager wields no subtree authority. Re-grant a live
        // permission to ic, pause lead, and confirm the revoke is denied on authority
        // (checked before holding) — ic keeps the permission.
        k.grant_permission_to_agent(&ic, perm.clone()).unwrap();
        k.update_agent(&lead, None, None, None, None, Some(AgentStatus::Paused))
            .unwrap();
        let err = k
            .manager_revoke_permission_from_subordinate_as_agent("agt_r1", &lead, &ic, &perm)
            .unwrap_err();
        assert!(matches!(err, KernelError::PermissionDenied { .. }), "paused manager denied: {err:?}");
        assert!(
            k.agent_holds_permission(&ic, &perm),
            "a paused manager's denied revoke must not touch ic's permission"
        );

        // A token-authenticated denial was audited.
        assert!(k.audit_log().iter().any(|e| {
            e.action == "agent:token_authenticated_manager_revoke_permission"
                && e.result == AuditResult::Denied
        }));
    }

    #[test]
    fn update_agent_applies_fields_persists_and_audits() {
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let id = k
            .create_agent("editme", "Edit Me", "first role", &adapter, &ns, None, vec![])
            .unwrap();

        // Edit name/description/persona/status; leave the adapter unchanged (None).
        k.update_agent(
            &id,
            Some("Edited Name".to_string()),
            Some("new role".to_string()),
            Some(Some("calm and precise".to_string())),
            None,
            Some(AgentStatus::Paused),
        )
        .unwrap();

        let agent = k.agent(&id).unwrap();
        assert_eq!(agent.name, "Edited Name");
        assert_eq!(agent.description, "new role");
        assert_eq!(agent.persona.as_deref(), Some("calm and precise"));
        assert_eq!(agent.status, AgentStatus::Paused);
        // The adapter was left untouched.
        assert_eq!(agent.adapter_plugin.as_str(), "relux-adapter-local-prime");
        assert!(k
            .audit_log()
            .iter()
            .any(|e| e.action == "agent:update" && e.result == AuditResult::Success));

        // Persona can be cleared with Some(None).
        k.update_agent(&id, None, None, Some(None), None, None).unwrap();
        assert!(k.agent(&id).unwrap().persona.is_none());
    }

    #[test]
    fn update_agent_rejects_unknown_agent_and_unknown_adapter() {
        let (mut k, prime, _task, _run, _echo) = primed_kernel();

        let unknown = AgentId::new("ghost");
        let err = k
            .update_agent(&unknown, Some("x".to_string()), None, None, None, None)
            .unwrap_err();
        assert!(matches!(err, KernelError::UnknownAgent(_)), "got {err:?}");

        // An adapter that is not an installed plugin is rejected; the agent is untouched.
        let bogus = PluginId::new("relux-adapter-not-installed");
        let err = k
            .update_agent(&prime, None, None, None, Some(bogus), None)
            .unwrap_err();
        assert!(matches!(err, KernelError::UnknownPlugin(_)), "got {err:?}");
    }

    #[test]
    fn create_agent_stores_lead_and_rejects_unknown_and_self() {
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let lead = k
            .create_agent("lead", "Lead", "the boss", &adapter, &ns, None, vec![])
            .unwrap();

        // A valid Lead is stored on the new operative.
        let ic = k
            .create_agent_with_skills(
                "ic", "IC", "ic role", &adapter, &ns, None, vec![], vec![], Some(lead.clone()),
            )
            .unwrap();
        assert_eq!(k.agent(&ic).unwrap().reports_to.as_ref(), Some(&lead));

        // An unknown manager is rejected (and nothing is created).
        let err = k
            .create_agent_with_skills(
                "ghosted", "G", "", &adapter, &ns, None, vec![], vec![],
                Some(AgentId::new("nobody")),
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidAgentConfig(_)), "got {err:?}");
        assert!(k.agent(&AgentId::new("ghosted")).is_none());

        // Reporting to your own id at creation is a self-report.
        let err = k
            .create_agent_with_skills(
                "selfie", "S", "", &adapter, &ns, None, vec![], vec![],
                Some(AgentId::new("selfie")),
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidAgentConfig(_)), "got {err:?}");
    }

    #[test]
    fn update_agent_sets_clears_and_rejects_lead_cycles() {
        let (mut k, _prime, _task, _run, _echo) = primed_kernel();
        let ns = NamespaceId::new("workspace");
        let adapter = PluginId::new("relux-adapter-local-prime");
        let lead = k
            .create_agent("lead", "Lead", "", &adapter, &ns, None, vec![])
            .unwrap();
        let ic = k
            .create_agent("ic", "IC", "", &adapter, &ns, None, vec![])
            .unwrap();

        // Set ic's Lead to lead.
        k.update_agent_with_skills(
            &ic, None, None, None, None, None, None, Some(Some(lead.clone())),
        )
        .unwrap();
        assert_eq!(k.agent(&ic).unwrap().reports_to.as_ref(), Some(&lead));

        // Pointing lead -> ic now would close lead -> ic -> lead: rejected as a cycle.
        let err = k
            .update_agent_with_skills(
                &lead, None, None, None, None, None, None, Some(Some(ic.clone())),
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidAgentConfig(_)), "got {err:?}");
        // lead is unchanged (still top-level).
        assert!(k.agent(&lead).unwrap().reports_to.is_none());

        // A self-report is rejected too.
        let err = k
            .update_agent_with_skills(
                &ic, None, None, None, None, None, None, Some(Some(ic.clone())),
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidAgentConfig(_)), "got {err:?}");

        // Clearing the Lead (Some(None)) returns ic to top-level.
        k.update_agent_with_skills(&ic, None, None, None, None, None, None, Some(None))
            .unwrap();
        assert!(k.agent(&ic).unwrap().reports_to.is_none());
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
    fn cancel_run_marks_run_cancelled_with_cancelled_class() {
        // The operator-cancel finalize (`docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §8/§26):
        // a running run becomes terminal Cancelled with the Cancelled failure class,
        // no retry, and a `run_cancelled` transcript event — distinct from a failure.
        let mut k = adapter_kernel();
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.start_run(&task).expect("start run");
        assert_eq!(k.run(&run_id).unwrap().status, RunStatus::Running);

        k.cancel_run(&run_id, "adapter 'claude' was cancelled by operator")
            .expect("cancel ok");

        let run = k.run(&run_id).unwrap();
        assert_eq!(
            run.status,
            RunStatus::Cancelled,
            "a cancel is terminal-Cancelled, not Failed"
        );
        assert_eq!(run.failure_class, Some(RunFailureClass::Cancelled));
        assert!(run.retry.is_none(), "a cancel is never auto-retried");
        assert!(run.ended_at.is_some());
        assert!(
            k.run_events(&run_id).iter().any(|e| e.kind == "run_cancelled"),
            "the transcript must record the intentional stop"
        );
        // Recovery projections never treat a cancel as a retry / operator-action
        // failure (they key on RunStatus::Failed; a Cancelled run is excluded).
        assert!(!k.transient_retry_ready(u64::MAX).contains(&run_id));
        let cancel_audit = k
            .audit_log()
            .iter()
            .any(|e| e.action == "run:cancel");
        assert!(cancel_audit, "a cancel is audited");
    }

    #[test]
    fn capture_cli_run_log_marks_a_cancelled_outcome() {
        // A cancelled adapter outcome captures an honest "cancelled by operator"
        // system line in the durable run-log tail (§8/§26).
        let mut k = adapter_kernel();
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.start_run(&task).expect("start run");
        let outcome = crate::adapter::AdapterRunOutcome {
            program: "fake".to_string(),
            exit_code: None,
            success: false,
            timed_out: false,
            cancelled: true,
            stdout: "partial output\n".to_string(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            duration_ms: 1234,
        };
        k.capture_cli_run_log(&run_id, &AdapterKind::ClaudeCli, "fake", &outcome);
        let log = k.run_log(&run_id, None);
        assert!(
            log.lines.iter().any(|l| l.source == relux_core::RunLogSource::System
                && l.text.contains("cancelled by operator")),
            "missing cancellation outcome line: {:?}",
            log.lines
        );
        // The captured partial stdout is still present (a cancel still shows output).
        assert!(log
            .lines
            .iter()
            .any(|l| l.source == relux_core::RunLogSource::Stdout
                && l.text.contains("partial output")));
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
    fn cli_run_captures_a_bounded_redacted_stdout_log_tail() {
        // A successful CLI run captures a bounded run-log tail: a system spawn
        // line, the stdout line(s), and a system outcome line — all redacted.
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-claude", "RAN_OK_42");
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        let log = k.run_log(&run_id, None);
        assert!(!log.is_empty(), "a CLI run must capture a log tail");
        // The stdout body is present and classified as stdout.
        assert!(log
            .lines
            .iter()
            .any(|l| l.source == relux_core::RunLogSource::Stdout && l.text.contains("RAN_OK_42")));
        // The kernel framed it with system spawn + outcome lines.
        let system: Vec<&str> = log
            .lines
            .iter()
            .filter(|l| l.source == relux_core::RunLogSource::System)
            .map(|l| l.text.as_str())
            .collect();
        assert!(system.iter().any(|t| t.contains("spawned")));
        assert!(system.iter().any(|t| t.contains("exited with code")));
    }

    #[test]
    fn cli_run_log_classifies_stderr_on_a_failing_run() {
        // A non-zero exit still captures a log; the "boom" line is on stderr.
        let dir = tempfile::tempdir().unwrap();
        let fake = write_failing_cli(dir.path(), "fake-fail");
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let _ = k.execute_assigned_run(&task); // expected to fail
        let run_id = k
            .runs()
            .iter()
            .find(|r| r.task_id == task)
            .map(|r| r.id.clone())
            .expect("a run exists");

        let log = k.run_log(&run_id, None);
        assert!(log
            .lines
            .iter()
            .any(|l| l.source == relux_core::RunLogSource::Stderr && l.text.contains("boom")));
    }

    #[test]
    fn run_log_since_cursor_returns_only_the_tail_and_empty_for_no_log() {
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-claude", "LINE_BODY");
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        let full = k.run_log(&run_id, None);
        let last = full.latest_seq().expect("non-empty log");
        // A cursor at the last seq returns no new lines but keeps the markers.
        let tail = k.run_log(&run_id, Some(last));
        assert!(tail.lines.is_empty());
        assert_eq!(tail.run_id, run_id);
        // A real run with NO captured log (the local-prime echo path) returns an
        // empty tail, never an error — the UI's "No logs" state.
        let (mut k2, _prime, echo_task, echo_run, _echo) = primed_kernel();
        let _ = k2.execute_assigned_run(&echo_task).expect("echo ok");
        let echo_log = k2.run_log(&echo_run, None);
        assert!(echo_log.is_empty());
        assert_eq!(echo_log.dropped_lines, 0);
    }

    #[test]
    fn captured_run_logs_survive_a_snapshot_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-claude", "PERSIST_ME");
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        let before = k.run_log(&run_id, None);
        assert!(!before.is_empty());
        let restored = KernelState::from_snapshot(k.snapshot());
        let after = restored.run_log(&run_id, None);
        assert_eq!(after, before, "run log must survive a snapshot round-trip");
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

    /// Write a fake CLI that echoes its own argv back to stdout (prefixed so the
    /// test can find it). Used to confirm the kernel threaded `--resume <id>` into
    /// the resume spawn — the fake ignores stdin and just reflects the args.
    fn write_fake_args_cli(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            std::fs::write(&path, "@echo off\r\necho ARGV: %*\r\n").unwrap();
            path
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\necho \"ARGV: $*\"\n").unwrap();
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            path
        }
    }

    #[test]
    fn cli_run_captures_provider_session_identity() {
        // A Claude-style envelope with a `session_id` records a bounded, resumable
        // RunSession on the run (the durable handoff metadata).
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"type":"result","is_error":false,"result":"did it","session_id":"sess-ABC-123"}"#;
        let fake = write_fake_json_cli(dir.path(), "fake-claude-sess", body);
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");

        let session = k.run(&run_id).unwrap().session.clone().expect("session captured");
        assert_eq!(session.adapter_session_id, "sess-ABC-123");
        assert_eq!(session.source, "claude-cli");
        assert!(session.resume_supported);
    }

    #[test]
    fn resume_run_refuses_a_run_without_a_session() {
        // A completed run that captured no provider session cannot be resumed —
        // refused honestly (no faked continuation), not turned into a silent run.
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(dir.path(), "fake-plain", "plain done");
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);
        let run_id = k.execute_assigned_run(&task).expect("adapter run ok");
        assert!(k.run(&run_id).unwrap().session.is_none());

        let err = k.resume_run(&run_id).unwrap_err();
        match err {
            KernelError::RunResumeNotSupported { reason, .. } => {
                assert!(reason.contains("no provider session"));
            }
            other => panic!("expected RunResumeNotSupported, got {other:?}"),
        }
    }

    #[test]
    fn resume_run_continues_a_captured_session_and_threads_the_resume_flag() {
        // First run captures a Claude session; resume then continues it through the
        // SAME governed gate, threading `--resume <session_id>` into the spawn and
        // stamping `resumed_from` lineage (distinct from a fresh retry).
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"type":"result","is_error":false,"result":"first","session_id":"sess-XYZ-9"}"#;
        let fake_json = write_fake_json_cli(dir.path(), "fake-claude-sess2", body);
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake_json);
        let (_agent, task) = cli_task(&mut k);
        let first = k.execute_assigned_run(&task).expect("first run ok");
        assert!(k.run(&first).unwrap().session.is_some());

        // Swap the binary for one that reflects its argv, so we can prove the
        // kernel threaded `--resume sess-XYZ-9` (still the claude_cli kind).
        let fake_args = write_fake_args_cli(dir.path(), "fake-claude-args");
        enable_claude_with(&mut k, &fake_args);

        let second = k.resume_run(&first).expect("resume ok");
        assert_ne!(second, first);
        let run2 = k.run(&second).unwrap();
        assert_eq!(run2.resumed_from.as_ref(), Some(&first));
        assert!(run2.retried_from.is_none(), "resume is not a retry");

        // The argv the kernel actually spawned contains `--resume <id>`.
        let stdout = k
            .run_events(&second)
            .iter()
            .rev()
            .find(|e| e.kind == "adapter_output")
            .and_then(|e| e.payload.get("stdout"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        assert!(stdout.contains("--resume"), "argv missing --resume: {stdout}");
        assert!(stdout.contains("sess-XYZ-9"), "argv missing session id: {stdout}");

        // The resume is recorded on the transcript + audit (honest provenance).
        assert!(k.run_events(&second).iter().any(|e| e.kind == "run_resumed_from"));
        assert!(k.audit_log().iter().any(|e| e.action == "run:resume"));
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
    fn a_transient_failure_schedules_a_bounded_retry() {
        // A transient (provider) failure records a retryable class and schedules a
        // bounded retry; the run is retry-ready only at/after its `not_before`.
        let (mut k, _agent, task, run, _echo) = primed_kernel();
        k.fail_run(&run, "provider returned 503 service unavailable")
            .unwrap();
        let r = k.run(&run).unwrap();
        assert_eq!(r.status, RunStatus::Failed);
        assert_eq!(r.failure_class, Some(RunFailureClass::TransientProvider));
        let retry = r.retry.as_ref().expect("a transient failure is retry-scheduled");
        assert_eq!(retry.attempt, 0);
        assert_eq!(retry.max_attempts, relux_core::MAX_TRANSIENT_RETRIES);
        assert!(!retry.exhausted);
        let nb = retry.not_before_secs.expect("a scheduled retry has a not-before");

        // Not eligible before the backoff elapses; eligible at/after it.
        assert!(
            k.transient_retry_ready(0).is_empty(),
            "a freshly-failed transient is not eligible immediately"
        );
        assert!(k.transient_retry_ready(nb).iter().any(|id| id == &run));

        // Doctor projections: a pending retry, nothing needing operator action.
        assert_eq!(k.runs_retry_pending(), 1);
        assert_eq!(k.runs_needing_operator_action(), 0);
        let _ = task;
    }

    #[test]
    fn a_non_retryable_failure_records_a_class_but_never_auto_retries() {
        // An auth / permission / config failure is classified but NEVER scheduled
        // for an automatic retry — it waits for an operator.
        for (reason, class) in [
            (
                "permission denied: agent prime lacks plugin:install",
                RunFailureClass::PermissionDenied,
            ),
            ("401 Unauthorized: invalid api key", RunFailureClass::AuthRequired),
        ] {
            let (mut k, _agent, _task, run, _echo) = primed_kernel();
            k.fail_run(&run, reason).unwrap();
            let r = k.run(&run).unwrap();
            assert_eq!(r.failure_class, Some(class));
            assert!(r.retry.is_none(), "{class:?} must not schedule a retry");
            assert!(
                k.transient_retry_ready(u64::MAX).is_empty(),
                "{class:?} is never retry-ready, even far in the future"
            );
            assert_eq!(k.runs_needing_operator_action(), 1);
            assert_eq!(k.runs_retry_pending(), 0);
        }
    }

    #[test]
    fn transient_retry_attempt_grows_and_exhausts_on_the_bounded_schedule() {
        // A CLI that always emits a transient (rate-limit) error envelope: every
        // attempt classifies as a retryable transient, so the bounded `[2m,10m,
        // 30m,2h]` budget grows attempt-by-attempt and exhausts after 4 retries.
        let dir = tempfile::tempdir().unwrap();
        let fake = write_fake_cli(
            dir.path(),
            "fake-transient",
            r#"{"type":"result","is_error":true,"result":"rate limit reached"}"#,
        );
        let mut k = adapter_kernel();
        enable_claude_with(&mut k, &fake);
        let (_agent, task) = cli_task(&mut k);

        let _ = k.execute_assigned_run(&task).unwrap_err();
        let first = k.runs().into_iter().next_back().unwrap();
        assert_eq!(first.failure_class, Some(RunFailureClass::TransientProvider));
        assert_eq!(first.retry.as_ref().unwrap().attempt, 0);
        let mut latest = first.id.clone();

        // Retry up to the cap; each re-attempt's lineage depth (and thus backoff
        // index) grows by one until the budget is exhausted.
        for expected_attempt in 1..=relux_core::MAX_TRANSIENT_RETRIES {
            let _ = k.retry_run(&latest).unwrap_err(); // same envelope still fails
            let newest = k.runs().into_iter().next_back().unwrap();
            assert_eq!(newest.retried_from.as_ref(), Some(&latest));
            let retry = newest.retry.as_ref().expect("still classified transient");
            assert_eq!(retry.attempt, expected_attempt);
            if expected_attempt < relux_core::MAX_TRANSIENT_RETRIES {
                assert!(!retry.exhausted);
                assert!(retry.not_before_secs.is_some());
            } else {
                // Past the bounded schedule: exhausted, no further auto-retry.
                assert!(retry.exhausted, "the transient budget must exhaust at the cap");
                assert!(retry.not_before_secs.is_none());
                assert!(
                    k.transient_retry_ready(u64::MAX).is_empty(),
                    "an exhausted run is never retry-ready"
                );
            }
            latest = newest.id.clone();
        }
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
    fn prime_run_orchestration_runs_an_existing_batch_by_id() {
        // The `RunOrchestration` Prime action runs the EXISTING governed batch: create an
        // orchestration, then ask Prime to run it by id → the briefs run and the record
        // moves to completed (the same `run_orchestration` engine the CLI/API use).
        let (mut k, ctx) = orchestration_kernel();
        let id = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap()
            .id
            .clone();
        assert_eq!(k.run_count(), 0, "nothing ran at create time");

        let turn = k.prime_turn(&ctx, &format!("run {id}")).unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::OrchestrationRun);
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        match turn.action {
            Some(PrimeAction::RunOrchestration { orchestration_id }) => {
                assert_eq!(orchestration_id, id.0)
            }
            other => panic!("expected RunOrchestration, got {other:?}"),
        }
        // The durable record really advanced — both briefs completed.
        let stored = k.orchestration(&id).unwrap();
        assert_eq!(stored.status, OrchestrationStatus::Completed);
        assert!(stored.steps.iter().all(|s| s.outcome == StepOutcome::Completed));
    }

    #[test]
    fn prime_run_orchestration_unknown_id_is_an_honest_reply() {
        // An explicit id that names no orchestration fails closed: an honest, action-free
        // reply — never a faked run.
        let (mut k, ctx) = orchestration_kernel();
        let turn = k.prime_turn(&ctx, "run orch_9999").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::OrchestrationRun);
        assert_eq!(turn.disposition, PrimeDisposition::Answered);
        assert!(turn.action.is_none(), "no action runs for an unknown id");
        assert!(turn.reply.to_lowercase().contains("no orchestration"));
    }

    #[test]
    fn prime_run_orchestration_without_an_id_clarifies() {
        // No id named → a resolvable clarify the multi-turn memory can continue.
        let (mut k, ctx) = orchestration_kernel();
        let _ = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap();
        let turn = k.prime_turn(&ctx, "run the orchestration").unwrap();
        assert_eq!(turn.intent, relux_core::PrimeIntent::OrchestrationRun);
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.action.is_none());
    }

    #[test]
    fn an_orchestration_run_clarification_is_resolved_by_an_id_follow_up() {
        // The canonical multi-turn dialogue: "run the orchestration" → "which one?" →
        // "orch_0001" continues the original request into a real batch run.
        let (mut k, ctx) = orchestration_kernel();
        let id = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap()
            .id
            .clone();

        let clarify = k.prime_turn(&ctx, "run the orchestration").unwrap();
        assert_eq!(clarify.disposition, PrimeDisposition::NeedsClarification);

        let resolved = k.prime_turn(&ctx, id.as_str()).unwrap();
        assert_eq!(resolved.intent, relux_core::PrimeIntent::OrchestrationRun);
        assert_eq!(resolved.disposition, PrimeDisposition::Executed);
        match resolved.action {
            Some(PrimeAction::RunOrchestration { orchestration_id }) => {
                assert_eq!(orchestration_id, id.0)
            }
            other => panic!("expected RunOrchestration, got {other:?}"),
        }
        assert_eq!(
            k.orchestration(&id).unwrap().status,
            OrchestrationStatus::Completed
        );
    }

    #[test]
    fn write_tool_orchestration_start_promotes_a_validated_id() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        // The `orchestration.start` write tool promotes an under-specified run request: the
        // message named no id (deterministic clarify), but a validated brain slot — the id
        // existence- AND runnability-checked against the live records — promotes it to the
        // SAME safe `RunOrchestration` action.
        let (mut k, ctx) = orchestration_kernel();
        let id = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap()
            .id
            .clone();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.start",
            "args": {"orchestration_id": id.0}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::RunOrchestration(run) = &req.slot else {
            panic!("expected a run-orchestration slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "run the orchestration",
                Some(&intent),
                BrainSlotProposals {
                    run_orchestration: Some(run),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::Executed);
        match turn.action {
            Some(PrimeAction::RunOrchestration { orchestration_id }) => {
                assert_eq!(orchestration_id, id.0)
            }
            other => panic!("expected RunOrchestration, got {other:?}"),
        }
        assert_eq!(
            k.orchestration(&id).unwrap().status,
            OrchestrationStatus::Completed
        );
    }

    #[test]
    fn write_tool_orchestration_start_fails_closed_on_an_unknown_id() {
        use crate::prime_write_tools::{parse_write_tool_request, WriteToolSlot};
        // A brain-proposed id that names no orchestration never resolves — the deterministic
        // clarify stands and nothing runs (fail closed).
        let (mut k, ctx) = orchestration_kernel();
        let _ = k
            .prime_orchestrate(&ctx, "implement the feature and document the result")
            .unwrap();
        let req = parse_write_tool_request(&serde_json::json!({
            "tool": "orchestration.start",
            "args": {"orchestration_id": "orch_9999"}
        }))
        .unwrap();
        let intent = req.intent_proposal();
        let WriteToolSlot::RunOrchestration(run) = &req.slot else {
            panic!("expected a run-orchestration slot");
        };
        let (turn, _) = k
            .prime_turn_with_brain(
                &ctx,
                "run the orchestration",
                Some(&intent),
                BrainSlotProposals {
                    run_orchestration: Some(run),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(turn.disposition, PrimeDisposition::NeedsClarification);
        assert!(turn.action.is_none());
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
