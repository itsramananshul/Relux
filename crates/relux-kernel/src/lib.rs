//! Relux kernel - the first local, deterministic control-plane loop.
//!
//! This crate sits one layer above `relux-core`: where `relux-core` defines the
//! canonical domain types (Namespace, Agent, Task, Run, Plugin, Permission,
//! Audit), `relux-kernel` provides an in-memory [`KernelState`] that stores them
//! and a minimal set of kernel actions that move work through the MVP loop from
//! `docs/RELUX_MASTER_PLAN.md` section 14:
//!
//! ```text
//! load plugin manifests -> create namespace -> create Prime agent ->
//! create task -> start run -> call tool (permission-checked) ->
//! complete run -> complete task, with an audit trail throughout.
//! ```
//!
//! Everything here is local-only and deterministic: no network, no wall clock,
//! no real API calls. It is the seam that ServiceProvider / Adapter / ToolSet
//! plugins will later sit behind.

pub mod adapter;
pub mod agent_auth;
pub mod agent_config;
pub mod agent_presets;
pub mod ai;
pub mod auth;
pub mod builtin;
pub mod clock;
pub mod doctor;
pub mod event;
pub mod introspect;
pub mod live_run_log;
pub mod loader;
pub mod mcp;
pub mod mcp_proposal;
pub mod mcp_stdio;
pub mod plugin_install;
pub mod plugin_tool_config;
pub mod prime;
pub mod prime_after_action;
pub mod prime_admin_slots;
pub mod prime_agent_slots;
pub mod prime_agent_loop;
pub mod prime_assign_slots;
pub mod prime_clarify;
pub mod prime_clarify_memory;
pub mod prime_decision;
pub mod prime_history;
pub mod prime_intent;
pub mod prime_orchestration_slots;
pub mod prime_slots;
pub mod prime_tools;
pub mod prime_update_slots;
pub mod prime_write_tools;
pub mod run_cancel;
pub mod runtime;
pub mod secret_store;
pub mod state;
pub mod store;


pub use adapter::{
    build_adapter_args, build_resume_adapter_args, compose_prompt, find_on_path,
    run_adapter_command, run_adapter_command_streaming, AdapterCommandSpec, AdapterRunOutcome,
};
pub use live_run_log::{LiveRunLogs, RunLogSink};
pub use run_cancel::{CancelOutcome, CancelState, CancelToken, RunCancellations};
pub use agent_config::{
    validate_agent_update, validate_new_agent, AgentConfigError, CreateAgentInput,
    ResolvedAgentUpdate, ResolvedNewAgent, UpdateAgentInput,
};
pub use agent_presets::{find_agent_preset, AgentPreset, AGENT_PRESETS};
pub use ai::{
    classify_intent_via_openrouter, clear_stored_config, complete_tool_round, compose_chat_prompt,
    compose_polish_prompt, decide_prime_via_openrouter, grounded_facts_with_observations,
    extract_agent_slots_via_openrouter, extract_assign_slots_via_openrouter,
    extract_permission_slots_via_openrouter,
    extract_plugin_ref_via_openrouter, extract_task_slots_via_openrouter,
    extract_update_slots_via_openrouter, is_actionful, polish_after_action_via_openrouter,
    polish_clarify_via_openrouter, polish_from_cli_text, polish_proposal, proposal_wants_polish,
    read_stored_config, shape_reply, write_stored_config, AiConfig, AiMode, AiOutcome, AiStatus,
    PrimeBrain, StoredAiConfig,
};
pub use agent_auth::{
    bearer_token_from_headers, AgentTokenIdentity, AgentTokenMeta, AgentTokenStore,
    MintedAgentToken, AGENT_TOKEN_DEFAULT_TTL_SECS, AGENT_TOKEN_MAX_TTL_SECS,
    AGENT_TOKEN_MIN_TTL_SECS, AGENT_TOKEN_PREFIX,
};
pub use auth::{
    admin_path_for_db, clear_session_cookie, read_admin_username, reset_admin_credential,
    session_cookie_from_headers, session_path_for_admin, set_session_cookie,
    set_session_cookie_with_max_age, ChangePasswordError, DashboardAuth, SessionMeta,
    MIN_PASSWORD_LEN, SESSION_ABSOLUTE_MAX_SECS, SESSION_COOKIE, SESSION_TTL_SECS,
};
pub use builtin::{is_builtin_tool, is_internal_plugin, BUILTIN_TOOLS, INTERNAL_PLUGIN_IDS};
pub use clock::Clock;
pub use event::RunEvent;
pub use introspect::{detect_hints, PluginHint};
pub use mcp_proposal::{propose_mcp_registration, McpRegistrationProposal};
pub use loader::{load_plugin_manifests, MANIFEST_FILENAME};
pub use plugin_install::{
    install_from_dir, install_from_github, install_from_zip, is_generated_manifest, list_installed,
    refresh_bundled_plugins, remove_plugin, GENERATED_MANIFEST_AUTHOR,
};
pub use plugin_tool_config::{parse_plugin_tool_input, PluginToolInput};
pub use prime::{
    clarify_needs_label, classify_intent, decide, is_chat_guarded, is_standalone_request,
};
pub use prime_admin_slots::{
    build_permission_slots_prompt, build_plugin_ref_prompt, parse_permission_slots,
    parse_plugin_ref, reconcile_permission_slots, reconcile_plugin_ref, BrainPermissionSlots,
    BrainPluginRef, ResolvedPermissionSlots,
};
pub use prime_agent_slots::{
    build_agent_slots_prompt, parse_agent_slots, reconcile_agent_slots, BrainAgentSlots,
    ResolvedAgentSlots,
};
pub use prime_after_action::{
    after_action_kind, build_action_envelope, build_after_action_prompt, parse_after_action,
    reconcile_after_action, ActionEnvelope, ActionFacts, ActionResultKind, BrainAfterAction,
};
pub use prime_clarify::{
    build_clarify_prompt, clarify_polish_kind, parse_clarify, reconcile_clarify, BrainClarify,
    ClarifyKind,
};
pub use prime_clarify_memory::{
    is_cancellation, is_resolvable_clarify_intent, resolve_pending, ClarifyResolution,
    CLARIFY_TTL_SECS,
};
pub use prime_decision::{
    build_decision_prompt, build_decision_prompt_with_correction, parse_decision, run_decision_loop,
    run_decision_loop_with_correction, DecisionLoop, DecisionOutcome, DecisionStep,
    PrimeBrainDecision, MAX_DECISION_CORRECTIONS, MAX_DECISION_ROUNDS,
};
pub use prime_history::{
    build_turn as build_history_turn, render_context as render_history_context, MAX_CONTEXT_CHARS,
    MAX_HISTORY_CONVERSATIONS, MAX_HISTORY_TURNS,
};
pub use prime_intent::{
    build_intent_prompt, parse_intent_proposal, reconcile_intent, BrainIntentProposal,
    IntentSource,
};
pub use prime_assign_slots::{
    build_assign_slots_prompt, parse_assign_slots, reconcile_assign_slots, BrainAssignSlots,
    ResolvedAssignSlots,
};
pub use prime_orchestration_slots::{
    build_orchestration_slots_prompt, parse_orchestration_slots, reconcile_orchestration_slots,
    BrainOrchestrationSlots, ResolvedOrchestration,
};
pub use prime_agent_loop::{
    build_agent_catalog, build_agent_prompt, interpret_agent_reply, prime_wants_extended_work,
    run_agent_loop, AgentExecStep, AgentLimits, AgentLoop, AgentLoopResult, AgentObservation,
    AgentOutcome, AgentPick, AgentReply, AgentStep, AgentTool, LimitKind, ToolStepOutcome,
};
pub use prime_slots::{
    build_task_slots_prompt, parse_task_slots, reconcile_task_slots, BrainTaskSlots,
    ResolvedTaskSlots,
};
pub use prime_tools::{
    build_tools_prompt, classify_tool, execute_context_tool, execute_requested_reads,
    execute_requested_reads_with_limit, interpret_reply, read_only_tool_names, reads_to_wire,
    render_observations, run_context_loop, run_context_loop_with_rounds, turn_wants_context,
    validate_tool_request, BrainTurn, ContextLoop, ContextRead, ContextSnapshot, ContextTool,
    ToolCall, ToolKind, MAX_TOOL_ROUNDS, MAX_TOOL_ROUNDS_CEIL, READ_ONLY_TOOLS,
};
pub use prime_update_slots::{
    build_update_slots_prompt, deterministic_update, parse_settable_status, parse_update_slots,
    reconcile_update_slots, BrainUpdateSlots, DeterministicUpdate, ResolvedTaskUpdate,
    TaskUpdatePatch,
};
pub use prime_write_tools::{
    classify_write_tool, parse_write_tool_request, reconcile_run_start, write_tool_names,
    BrainRunOrchestration, BrainRunStart, ParsedWriteTool, WriteTool, WriteToolSlot, WRITE_TOOLS,
    WRITE_TOOL_CONFIDENCE,
};
pub use mcp::{
    call_tool as call_mcp_tool, discover_tools as discover_mcp_tools, McpClientError,
};
pub use runtime::{invoke_http_loopback, RuntimeClientError};
pub use secret_store::{
    init_mcp_workspace_root, init_secret_store, mcp_workspace_root, resolve_managed_env_and_cwd,
    secret_store, validate_managed_cwd, SecretStore,
};
pub use state::{
    discover_proposal_mcp_catalog, run_briefs_in_parallel, run_briefs_in_parallel_streaming,
    AppliedProposedChange, AppliedProposedChangeSet, BrainSlotProposals, BundledRefresh,
    BundledRefreshSummary, ContinuationPause, FinishedBrief, KernelCounters, KernelSnapshot,
    KernelState, PendingClarificationEntry, PendingToolInvocation, PreparedBrief,
    PrimeAgentContinuationEntry, ProposalMcpCatalog, ProposalMcpServer, ProposalMcpTool, RoundPrep,
    MAX_PENDING_CLARIFICATIONS, MAX_PRIME_CONTINUATIONS, MAX_TOOL_INVOCATION_ARGS_BYTES,
};
pub use store::SqliteStore;

use relux_core::ManifestError;
use thiserror::Error;

/// Returns the relux-kernel crate version.
pub fn get_kernel_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Errors produced by the kernel and its manifest loader.
#[derive(Debug, Error)]
pub enum KernelError {
    #[error("io error at {path}: {message}")]
    Io { path: String, message: String },
    #[error("failed to parse manifest at {path}: {message}")]
    ManifestParse { path: String, message: String },
    #[error("invalid manifest at {path}: {source}")]
    ManifestInvalid {
        path: String,
        #[source]
        source: ManifestError,
    },
    #[error("unknown plugin: {0}")]
    UnknownPlugin(String),
    #[error("unknown agent: {0}")]
    UnknownAgent(String),
    #[error("agent already exists: {0}")]
    AgentExists(String),
    #[error("invalid agent configuration: {0}")]
    InvalidAgentConfig(String),
    #[error("unknown task: {0}")]
    UnknownTask(String),
    #[error("unknown run: {0}")]
    UnknownRun(String),
    #[error("no active run found for task: {0}")]
    NoActiveRun(String),
    #[error("run {run} is not retryable (status {status}); only failed runs can be retried")]
    RunNotRetryable { run: String, status: String },
    #[error("run {run} cannot be resumed: {reason}")]
    RunResumeNotSupported { run: String, reason: String },
    #[error("run {run} has no proposed change at index {index}")]
    UnknownProposedChange { run: String, index: usize },
    #[error("proposed change {index} on run {run} is not approved (status {status}); approve it before applying")]
    ProposedChangeNotApproved {
        run: String,
        index: usize,
        status: String,
    },
    #[error("proposed change {index} on run {run} cannot be applied: {reason}")]
    ProposedChangeNotApplicable {
        run: String,
        index: usize,
        reason: String,
    },
    #[error("proposed change {index} on run {run} conflicts with the workspace: {reason}")]
    ProposedChangeConflict {
        run: String,
        index: usize,
        reason: String,
    },
    #[error("the proposed-change set on run {run} cannot be applied: {reason}")]
    ProposedChangeSetNotApplicable { run: String, reason: String },
    #[error("the proposed-change set on run {run} conflicts with the workspace: {reason}")]
    ProposedChangeSetConflict { run: String, reason: String },
    #[error("unknown approval: {0}")]
    UnknownApproval(String),
    #[error("unknown orchestration: {0}")]
    UnknownOrchestration(String),
    #[error("goal does not split into multiple briefs; it reads as a single task")]
    OrchestrationNotMultiAgent,
    #[error("task {0} has no assigned agent")]
    TaskNotAssigned(String),
    #[error("task {task} is not assignable in status {status}; only a non-terminal task can be (re)assigned")]
    TaskNotAssignable { task: String, status: String },
    #[error("plugin {plugin} has no tool named {tool}")]
    ToolNotFound { plugin: String, tool: String },
    #[error("tool {tool} on plugin {plugin} has no runtime handler yet (installed as metadata only; arbitrary plugin code is not executed)")]
    ToolRuntimeUnavailable { plugin: String, tool: String },
    #[error("plugin {plugin} has an HTTP loopback runtime configured but it is disabled")]
    ToolRuntimeDisabled { plugin: String },
    #[error("loopback runtime for {tool} on {plugin} failed: {message}")]
    ToolRuntimeInvocation {
        plugin: String,
        tool: String,
        message: String,
    },
    #[error("invalid tool runtime config for plugin {plugin}: {message}")]
    InvalidRuntimeConfig { plugin: String, message: String },
    #[error("no tool runtime configured for plugin {plugin}")]
    RuntimeNotConfigured { plugin: String },
    #[error("plugin {plugin} is not an Adapter plugin")]
    NotAnAdapter { plugin: String },
    #[error("adapter {plugin} cannot be configured as a CLI runtime: {message}")]
    AdapterNotConfigurable { plugin: String, message: String },
    #[error("invalid adapter runtime config for {plugin}: {message}")]
    InvalidAdapterConfig { plugin: String, message: String },
    #[error("no adapter runtime configured for {plugin}; enable it first (disabled by default)")]
    AdapterRuntimeNotConfigured { plugin: String },
    #[error("adapter runtime for {plugin} is configured but disabled")]
    AdapterRuntimeDisabled { plugin: String },
    #[error("adapter {plugin} binary '{binary}' was not found on PATH; install it or set an explicit command")]
    AdapterBinaryMissing { plugin: String, binary: String },
    #[error("adapter {plugin} run failed: {message}")]
    AdapterExecutionFailed { plugin: String, message: String },
    #[error("permission denied: agent {agent} lacks {permission}")]
    PermissionDenied { agent: String, permission: String },
    #[error("permission '{1}' already granted to agent {0}")]
    PermissionAlreadyGranted(String, String),
    #[error("agent {0} does not hold permission '{1}'")]
    PermissionNotGranted(String, String),
    #[error("storage error: {0}")]
    Storage(String),
    /// `serve` could not bind its listen address (e.g. a port conflict). The
    /// payload is already a complete, operator-facing message, so it is shown
    /// verbatim with no extra prefix.
    #[error("{0}")]
    ServeBind(String),
    #[error("plugin install failed: {0}")]
    PluginInstall(String),
    #[error("plugin not installed: {0}")]
    PluginNotInstalled(String),
    #[error("plugin {0} is bundled and cannot be removed")]
    BundledPluginProtected(String),
    #[error("plugin {plugin} cannot have tools configured this way: {message}")]
    PluginNotToolConfigurable { plugin: String, message: String },
    #[error("invalid tool definition for plugin {plugin}: {message}")]
    InvalidToolDefinition { plugin: String, message: String },
    #[error("plugin {plugin} has no configured tool named {tool}")]
    PluginToolNotFound { plugin: String, tool: String },
    #[error("tool {tool} on plugin {plugin} requires approval and cannot be invoked directly yet")]
    ToolRequiresApproval { plugin: String, tool: String },
    #[error("tool {tool} on plugin {plugin} does not require approval; enable a loopback runtime and invoke it directly")]
    ToolDoesNotRequireApproval { plugin: String, tool: String },
    #[error("tool invocation arguments for {tool} on {plugin} are too large: {size} bytes (max {max})")]
    ToolInvocationArgsTooLarge {
        plugin: String,
        tool: String,
        size: usize,
        max: usize,
    },
    #[error("approval {0} is not a tool-invocation approval (no bound invocation to execute)")]
    NoBoundToolInvocation(String),
    #[error("approval {id} is not approved (status {status}); decide it before executing")]
    ToolInvocationNotApproved { id: String, status: String },
    #[error("approval {0} has already been executed; request a new approval to run it again")]
    ToolInvocationConsumed(String),
    #[error("approval {0} is bound to a tool invocation whose stored arguments failed their integrity check")]
    ToolInvocationArgsTampered(String),
    #[error("persistent grant {0} does not exist")]
    UnknownPersistentGrant(String),
    #[error("unsafe plugin path rejected: {0}")]
    UnsafePluginPath(String),
    #[error("invalid MCP server config for '{id}': {message}")]
    InvalidMcpConfig { id: String, message: String },
    #[error("unknown MCP server: {0}")]
    UnknownMcpServer(String),
    #[error("MCP server '{0}' is disabled; enable it before discovering its tools")]
    McpServerDisabled(String),
    #[error("MCP server '{0}' is not a managed-stdio server; only managed-stdio servers have a process lifecycle")]
    NotAManagedStdioServer(String),
    #[error("MCP discovery against server '{id}' failed: {message}")]
    McpDiscoveryFailed { id: String, message: String },
    #[error("invalid MCP tool name '{tool}' for server '{server}' (must be [A-Za-z0-9._-], non-empty, bounded)")]
    InvalidMcpToolName { server: String, tool: String },
    #[error("invalid MCP resource URI '{uri}' for server '{server}' (must be non-empty, bounded, control-char free)")]
    InvalidMcpResourceUri { server: String, uri: String },
    #[error("MCP resource fetch against server '{id}' failed: {message}")]
    McpResourceFetchFailed { id: String, message: String },
}
