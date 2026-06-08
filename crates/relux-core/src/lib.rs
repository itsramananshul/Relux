//! Relux core domain types — Namespace, Agent, Task, Run, Plugin, Permissions, Audit, Prime.
//!
//! This crate defines the canonical data model described in
//! `docs/RELUX_MASTER_PLAN.md` §9 (Core Entities) and §7.1–7.5 (Product Layers).
//! No runtime, no storage, no dashboard — pure types + validation.

pub mod agent;
pub mod audit;
pub mod namespace;
pub mod permission;
pub mod plugin;
pub mod prime;
pub mod run;
pub mod task;

pub use agent::{Agent, AgentId};
pub use audit::{AuditEvent, AuditResult};
pub use namespace::{Namespace, NamespaceId};
pub use permission::{ApprovalRequirement, Permission, PermissionError, RiskLevel, ToolDefinition};
pub use plugin::{
    ManifestError, PluginCapability, PluginHealth, PluginId, PluginKind, PluginManifest, TrustLevel,
};
pub use prime::{PrimeAction, PrimeIntent};
pub use run::{Run, RunId, RunStatus};
pub use task::{Task, TaskId, TaskStatus};
