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
pub mod audit;
pub mod namespace;
pub mod orchestration;
pub mod permission;
pub mod plugin;
pub mod prime;
pub mod proposed_change;
pub mod redact;
pub mod run;
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
pub use namespace::{Namespace, NamespaceId};
pub use orchestration::{
    plan_orchestration, Orchestration, OrchestrationBatchResult, OrchestrationId,
    OrchestrationPlan, OrchestrationRole, OrchestrationStatus, OrchestrationStep, PlannedStep,
    StepOutcome,
};
pub use permission::{ApprovalRequirement, Permission, PermissionError, RiskLevel, ToolDefinition};
pub use plugin::{
    InstalledPlugin, ManifestError, PluginCapability, PluginHealth, PluginId, PluginKind,
    PluginManifest, PluginSourceKind, TrustLevel,
};
pub use prime::{
    PrimeAction, PrimeAutonomyConfig, PrimeAutonomyTickResult, PrimeContext, PrimeDisposition,
    PrimeIntent, PrimePlan, PrimeTurn, StateSummary, TaskBrief,
};
pub use proposed_change::{
    capture_proposed_changes, sha256_hex, ProposedChange, ProposedChangeAction,
    ProposedChangeStatus, MAX_CONTENT_BYTES, MAX_PROPOSED_CHANGES,
};
pub use redact::redact_secrets;
pub use run::{Run, RunId, RunStatus};
pub use runtime::{
    clamp_runtime_timeout, parse_loopback_url, validate_loopback_url, LoopbackUrl, LoopbackUrlError,
    RuntimeKind, ToolRuntimeConfig, DEFAULT_RUNTIME_TIMEOUT_MS, MAX_RUNTIME_TIMEOUT_MS,
    MIN_RUNTIME_TIMEOUT_MS,
};
pub use task::{Task, TaskId, TaskStatus};
pub use tool::{ToolDescriptor, ToolExecutability, ToolInvocationResult};
