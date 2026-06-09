//! Relux core domain types - Namespace, Agent, Task, Run, Plugin, Permissions, Audit, Prime.
//!
//! This crate defines the canonical data model described in
//! `docs/RELUX_MASTER_PLAN.md` section 9 (Core Entities) and section 7.1-7.5 (Product Layers).
//! No runtime, no storage, no dashboard - pure types + validation.

pub mod agent;
pub mod approval;
pub mod audit;
pub mod namespace;
pub mod permission;
pub mod plugin;
pub mod prime;
pub mod run;
pub mod runtime;
pub mod task;
pub mod tool;

pub use agent::{Agent, AgentId};
pub use approval::{Approval, ApprovalId, ApprovalStatus};
pub use audit::{AuditEvent, AuditResult};
pub use namespace::{Namespace, NamespaceId};
pub use permission::{ApprovalRequirement, Permission, PermissionError, RiskLevel, ToolDefinition};
pub use plugin::{
    InstalledPlugin, ManifestError, PluginCapability, PluginHealth, PluginId, PluginKind,
    PluginManifest, PluginSourceKind, TrustLevel,
};
pub use prime::{
    PrimeAction, PrimeAutonomyConfig, PrimeAutonomyTickResult, PrimeContext, PrimeDisposition,
    PrimeIntent, PrimePlan, PrimeTurn, StateSummary, TaskBrief,
};
pub use run::{Run, RunId, RunStatus};
pub use runtime::{
    clamp_runtime_timeout, parse_loopback_url, validate_loopback_url, LoopbackUrl, LoopbackUrlError,
    RuntimeKind, ToolRuntimeConfig, DEFAULT_RUNTIME_TIMEOUT_MS, MAX_RUNTIME_TIMEOUT_MS,
    MIN_RUNTIME_TIMEOUT_MS,
};
pub use task::{Task, TaskId, TaskStatus};
pub use tool::{ToolDescriptor, ToolExecutability, ToolInvocationResult};
