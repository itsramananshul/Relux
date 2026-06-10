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
pub mod ai;
pub mod builtin;
pub mod clock;
pub mod event;
pub mod loader;
pub mod plugin_install;
pub mod prime;
pub mod runtime;
pub mod state;
pub mod store;


pub use adapter::{
    build_adapter_args, compose_prompt, find_on_path, run_adapter_command, AdapterCommandSpec,
    AdapterRunOutcome,
};
pub use ai::{
    clear_stored_config, compose_chat_prompt, is_actionful, read_stored_config, shape_reply,
    write_stored_config, AiConfig, AiMode, AiOutcome, AiStatus, PrimeBrain, StoredAiConfig,
};
pub use builtin::{is_builtin_tool, is_internal_plugin, BUILTIN_TOOLS, INTERNAL_PLUGIN_IDS};
pub use clock::Clock;
pub use event::RunEvent;
pub use loader::{load_plugin_manifests, MANIFEST_FILENAME};
pub use plugin_install::{
    install_from_dir, install_from_github, install_from_zip, is_generated_manifest, list_installed,
    refresh_bundled_plugins, remove_plugin, GENERATED_MANIFEST_AUTHOR,
};
pub use prime::{classify_intent, decide};
pub use runtime::{invoke_http_loopback, RuntimeClientError};
pub use state::{BundledRefresh, BundledRefreshSummary, KernelCounters, KernelSnapshot, KernelState};
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
