//! Relux core domain types - Namespace, Agent, Task, Run, Plugin, Permissions, Audit, Prime.
//!
//! This crate defines the canonical data model described in
//! `docs/RELUX_MASTER_PLAN.md` section 9 (Core Entities) and section 7.1-7.5 (Product Layers).
//! No runtime, no storage, no dashboard - pure types + validation.

pub mod adapter;
pub mod adapter_result;
pub mod agent;
pub mod artifact;
pub mod approval;
pub mod hierarchy;
pub mod audit;
pub mod mcp;
pub mod namespace;
pub mod orchestration;
pub mod permission;
pub mod persistent_grant;
pub mod plugin;
pub mod prime;
pub mod proposed_change;
pub mod redact;
pub mod run;
pub mod run_failure;
pub mod run_log;
pub mod run_session;
pub mod runtime;
pub mod task;
pub mod tool;

pub use adapter::{
    clamp_adapter_max_output, clamp_adapter_timeout, recognize_adapter_kind, AdapterKind,
    AdapterRuntimeConfig, AdapterRuntimeState, AdapterRuntimeStatus, CLAUDE_CLI_ADAPTER_ID,
    CODEX_CLI_ADAPTER_ID, DEFAULT_ADAPTER_MAX_OUTPUT_BYTES, DEFAULT_ADAPTER_TIMEOUT_SECONDS,
    LOCAL_PRIME_ADAPTER_ID,
};
pub use adapter_result::{parse_adapter_result, AdapterResultSummary};
pub use agent::{Agent, AgentId};
pub use artifact::{capture_run_artifacts, ArtifactKind, RunArtifact, MAX_ARTIFACTS};
pub use approval::{Approval, ApprovalId, ApprovalStatus};
pub use audit::{AuditEvent, AuditResult};
pub use hierarchy::{
    chain_of_command, is_in_subtree, would_create_cycle, ReportsToMap, MAX_HIERARCHY_DEPTH,
};
pub use mcp::{
    clamp_mcp_timeout, is_valid_mcp_id, is_valid_mcp_resource_uri, is_valid_mcp_tool_name,
    mcp_synthetic_plugin_id, mcp_tool_permission, sanitize_mcp_resource_description,
    sanitize_mcp_server_id, sanitize_mcp_text, sanitize_mcp_tool_description, scan_mcp_tool_description,
    validate_mcp_server_config, validate_stdio_command, McpConfigError, McpResource,
    McpResourceContent, McpServerConfig,
    McpTool, McpToolClassification, McpTransport, DEFAULT_MCP_TIMEOUT_MS,
    MAX_MCP_ARGS, MAX_MCP_ARG_CHARS, MAX_MCP_COMMAND_CHARS,
    MAX_MCP_DESCRIPTION_CHARS, MAX_MCP_ID_CHARS, MAX_MCP_RESOURCES, MAX_MCP_RESOURCE_DESC_CHARS,
    MAX_MCP_RESOURCE_MIME_CHARS, MAX_MCP_RESOURCE_NAME_CHARS, MAX_MCP_RESOURCE_TEXT_CHARS,
    MAX_MCP_RESOURCE_URI_CHARS, MAX_MCP_TIMEOUT_MS, MAX_MCP_TOOLS, MAX_MCP_TOOL_DESC_CHARS,
    MAX_MCP_TOOL_NAME_CHARS, MIN_MCP_TIMEOUT_MS,
};
pub use namespace::{Namespace, NamespaceId};
pub use orchestration::{
    plan_orchestration, plan_orchestration_with_limit, Orchestration, OrchestrationBatchResult,
    OrchestrationId, OrchestrationPlan, OrchestrationRole, OrchestrationStatus, OrchestrationStep,
    PlannedStep, StepOutcome, EXTENDED_MAX_ORCHESTRATION_STEPS, MAX_ORCHESTRATION_STEPS,
    MAX_ORCHESTRATION_STEPS_CEIL,
};
pub use permission::{ApprovalRequirement, Permission, PermissionError, RiskLevel, ToolDefinition};
pub use persistent_grant::PersistentGrant;
pub use plugin::{
    InstalledPlugin, ManifestError, PluginCapability, PluginHealth, PluginId, PluginKind,
    PluginManifest, PluginSourceKind, TrustLevel,
};
pub use prime::{
    ConversationSummary, ConversationTurn, PendingClarification, PrimeAction, PrimeAdminSlots,
    PrimeAgentContinuation, PrimeAgentSlots,
    PrimeAgentLimits, PrimeAgentPolicy,
    PrimeAssignSlots,
    PrimeContinuationApproval, PrimeContinuationHandle, PrimeContinuationStep,
    PrimeAutonomyConfig, PrimeAutonomyTickResult, PrimeContext, PrimeContextRead, PrimeDisposition,
    PrimeIntent,
    PrimePlan, PrimePolishedStep, PrimeProposal, PrimeProposalPolish, PrimeProposalStep,
    PrimeSuggestion, PrimeTaskChange, PrimeTaskSlots, PrimeTaskUpdate, PrimeToolApprovalRequest,
    PrimeToolPlanProposal, PrimeToolTrace,
    PrimeToolPlanStep, PrimeTurn, StateSummary,
    TaskBrief,
};
pub use proposed_change::{
    capture_proposed_changes, sha256_hex, ProposedChange, ProposedChangeAction,
    ProposedChangeStatus, MAX_CONTENT_BYTES, MAX_PROPOSED_CHANGES,
};
pub use redact::redact_secrets;
pub use run::{Run, RunId, RunStatus};
pub use run_log::{
    RunLog, RunLogBuilder, RunLogLine, RunLogSource, StreamingRunLog, MAX_LOG_LINES,
    MAX_LOG_LINE_CHARS,
};
pub use run_failure::{
    classify_failure, retry_delay_secs, safe_public_message, RunFailureClass, RunRetryState,
    MAX_PUBLIC_MESSAGE_CHARS, MAX_TRANSIENT_RETRIES, RETRY_BACKOFF_SECS,
};
pub use run_session::{
    plan_resume, sanitize_session_id, ResumeDisposition, RunSession, MAX_SESSION_ID_LEN,
};
pub use runtime::{
    clamp_runtime_timeout, parse_loopback_url, validate_loopback_url, LoopbackUrl, LoopbackUrlError,
    RuntimeKind, ToolRuntimeConfig, DEFAULT_RUNTIME_TIMEOUT_MS, MAX_RUNTIME_TIMEOUT_MS,
    MIN_RUNTIME_TIMEOUT_MS,
};
pub use task::{
    parse_task_tool_call, parse_task_tool_plan, Task, TaskId, TaskStatus, TaskToolCall,
    TaskToolPlan, TaskToolPlanError, MAX_TASK_TOOL_PLAN_ARGS_BYTES, MAX_TASK_TOOL_PLAN_STEPS,
    MAX_TASK_TOOL_PLAN_STEPS_CEIL,
};
pub use tool::{
    approval_blocks_direct_invocation, ToolDescriptor, ToolExecutability, ToolInvocationResult,
};
