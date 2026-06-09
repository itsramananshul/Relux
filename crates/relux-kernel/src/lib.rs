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

pub mod ai;
pub mod builtin;
pub mod clock;
pub mod event;
pub mod loader;
pub mod plugin_install;
pub mod prime;
pub mod state;
pub mod store;


pub use ai::{shape_reply, AiConfig, AiMode, AiOutcome, AiStatus};
pub use builtin::{is_builtin_tool, BUILTIN_TOOLS};
pub use clock::Clock;
pub use event::RunEvent;
pub use loader::{load_plugin_manifests, MANIFEST_FILENAME};
pub use plugin_install::{
    install_from_dir, install_from_github, install_from_zip, list_installed, remove_plugin,
};
pub use prime::{classify_intent, decide};
pub use state::{KernelCounters, KernelSnapshot, KernelState};
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
    #[error("unknown task: {0}")]
    UnknownTask(String),
    #[error("unknown run: {0}")]
    UnknownRun(String),
    #[error("no active run found for task: {0}")]
    NoActiveRun(String),
    #[error("unknown approval: {0}")]
    UnknownApproval(String),
    #[error("task {0} has no assigned agent")]
    TaskNotAssigned(String),
    #[error("plugin {plugin} has no tool named {tool}")]
    ToolNotFound { plugin: String, tool: String },
    #[error("tool {tool} on plugin {plugin} has no runtime handler yet (installed as metadata only; arbitrary plugin code is not executed)")]
    ToolRuntimeUnavailable { plugin: String, tool: String },
    #[error("permission denied: agent {agent} lacks {permission}")]
    PermissionDenied { agent: String, permission: String },
    #[error("permission '{1}' already granted to agent {0}")]
    PermissionAlreadyGranted(String, String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("plugin install failed: {0}")]
    PluginInstall(String),
    #[error("plugin not installed: {0}")]
    PluginNotInstalled(String),
    #[error("plugin {0} is bundled and cannot be removed")]
    BundledPluginProtected(String),
    #[error("unsafe plugin path rejected: {0}")]
    UnsafePluginPath(String),
}
