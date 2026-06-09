//! Per-adapter runtime configuration for local coding-agent CLIs (Adapter
//! Runtime v1).
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 8.1 (Adapter Plugins) and
//! section 14 (the first plugin-powered run). An Adapter plugin connects Relux to
//! a model or agent runtime. The first real adapters are **local coding-agent
//! CLIs** the operator already has installed (Claude CLI, Codex CLI) plus a
//! generic local command shape. This module holds the pure config + recognition
//! types; the kernel `adapter` module holds the process spawn/probe logic.
//!
//! Safety rules pinned here (`docs/RELUX_MASTER_PLAN.md` section 17.5, the
//! product safety bar):
//!
//! - A CLI adapter runtime is **disabled by default**. Relux never spawns a
//!   paid/interactive CLI unless the operator explicitly enables it.
//! - This config carries NO secrets - only how to launch the local binary
//!   (kind/command), whether it is enabled, the timeout, the output cap, and an
//!   optional working directory.
//! - The local deterministic Prime adapter (`relux-adapter-local-prime`) is NOT
//!   configurable here: it has no external binary and always runs the in-memory
//!   echo path.

use serde::{Deserialize, Serialize};

/// Default per-run wall-clock timeout for a CLI adapter, in seconds.
pub const DEFAULT_ADAPTER_TIMEOUT_SECONDS: u64 = 120;
/// Lower clamp for an adapter timeout (a stray tiny value can't make every run
/// abort instantly).
pub const MIN_ADAPTER_TIMEOUT_SECONDS: u64 = 5;
/// Upper clamp for an adapter timeout (30 minutes - a stray huge value can't pin
/// a child process open indefinitely).
pub const MAX_ADAPTER_TIMEOUT_SECONDS: u64 = 1_800;

/// Default cap on captured stdout/stderr bytes (1 MiB).
pub const DEFAULT_ADAPTER_MAX_OUTPUT_BYTES: u64 = 1_000_000;
/// Lower clamp for the output cap (keep at least a small transcript).
pub const MIN_ADAPTER_MAX_OUTPUT_BYTES: u64 = 1_024;
/// Upper clamp for the output cap (16 MiB - refuse a runaway transcript).
pub const MAX_ADAPTER_MAX_OUTPUT_BYTES: u64 = 16_000_000;

/// The plugin id of the bundled Claude CLI adapter.
pub const CLAUDE_CLI_ADAPTER_ID: &str = "relux-adapter-claude-cli";
/// The plugin id of the bundled Codex CLI adapter.
pub const CODEX_CLI_ADAPTER_ID: &str = "relux-adapter-codex-cli";
/// The plugin id of the bundled local deterministic Prime adapter.
pub const LOCAL_PRIME_ADAPTER_ID: &str = "relux-adapter-local-prime";

/// The kind of runtime an Adapter plugin drives.
///
/// `LocalPrime` is the in-memory deterministic echo path; the other three spawn a
/// local CLI binary in a non-interactive, non-bypass mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    /// The local deterministic Prime adapter. No external process; runs the
    /// in-memory echo loop. Cannot have a CLI runtime configured.
    LocalPrime,
    /// Anthropic's Claude CLI (`claude -p ... --permission-mode default`).
    ClaudeCli,
    /// OpenAI's Codex CLI (`codex exec ...`).
    CodexCli,
    /// A generic local command adapter. Requires an explicit `command`; the
    /// composed prompt is passed as a single argument.
    Command,
}

impl AdapterKind {
    /// The stable wire string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            AdapterKind::LocalPrime => "local_prime",
            AdapterKind::ClaudeCli => "claude_cli",
            AdapterKind::CodexCli => "codex_cli",
            AdapterKind::Command => "command",
        }
    }

    /// The default binary name for a known CLI kind, or `None` when the kind has
    /// no built-in default (local-prime has no binary; generic command requires
    /// an explicit command).
    pub fn default_command(&self) -> Option<&'static str> {
        match self {
            AdapterKind::ClaudeCli => Some("claude"),
            AdapterKind::CodexCli => Some("codex"),
            AdapterKind::Command | AdapterKind::LocalPrime => None,
        }
    }

    /// True when this kind spawns a local CLI process (everything except the
    /// in-memory local-prime adapter).
    pub fn is_cli(&self) -> bool {
        !matches!(self, AdapterKind::LocalPrime)
    }
}

/// Recognize a well-known adapter kind from a plugin id. Returns `None` for an
/// unrecognized adapter plugin (which may still be driven as a generic
/// [`AdapterKind::Command`] once an operator configures a command for it).
pub fn recognize_adapter_kind(plugin_id: &str) -> Option<AdapterKind> {
    match plugin_id {
        LOCAL_PRIME_ADAPTER_ID => Some(AdapterKind::LocalPrime),
        CLAUDE_CLI_ADAPTER_ID => Some(AdapterKind::ClaudeCli),
        CODEX_CLI_ADAPTER_ID => Some(AdapterKind::CodexCli),
        _ => None,
    }
}

/// Durable, per-installed-adapter runtime configuration.
///
/// Persisted locally alongside the rest of the control plane. Carries no secrets:
/// only how to launch the local binary, whether it is enabled, the per-run
/// timeout, the output cap, and an optional working directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterRuntimeConfig {
    /// The installed adapter plugin this runtime backs.
    pub plugin_id: String,
    /// The kind of runtime (known CLI or generic command).
    pub kind: AdapterKind,
    /// Whether the runtime is enabled. **Disabled by default**; a disabled
    /// runtime is kept (so the command survives) but refuses execution.
    pub enabled: bool,
    /// An explicit binary name/path. Optional for a known kind (overrides the
    /// default); required for a generic [`AdapterKind::Command`].
    pub command: Option<String>,
    /// Per-run wall-clock timeout in seconds (already clamped).
    pub timeout_seconds: u64,
    /// Cap on captured stdout/stderr bytes (already clamped).
    pub max_output_bytes: u64,
    /// Optional working directory for the spawned process.
    pub working_dir: Option<String>,
}

impl AdapterRuntimeConfig {
    /// The binary that will actually be launched: the explicit `command` if set,
    /// otherwise the kind's default. `None` for a generic command with no command
    /// configured (an invalid state the kernel refuses to persist).
    pub fn resolved_command(&self) -> Option<String> {
        self.command
            .clone()
            .or_else(|| self.kind.default_command().map(str::to_string))
    }
}

/// Clamp a requested adapter timeout into the supported range, defaulting when
/// absent.
pub fn clamp_adapter_timeout(timeout_seconds: Option<u64>) -> u64 {
    timeout_seconds
        .unwrap_or(DEFAULT_ADAPTER_TIMEOUT_SECONDS)
        .clamp(MIN_ADAPTER_TIMEOUT_SECONDS, MAX_ADAPTER_TIMEOUT_SECONDS)
}

/// Clamp a requested output cap into the supported range, defaulting when absent.
pub fn clamp_adapter_max_output(max_output_bytes: Option<u64>) -> u64 {
    max_output_bytes
        .unwrap_or(DEFAULT_ADAPTER_MAX_OUTPUT_BYTES)
        .clamp(MIN_ADAPTER_MAX_OUTPUT_BYTES, MAX_ADAPTER_MAX_OUTPUT_BYTES)
}

/// The honest, current status of one installed adapter plugin's runtime.
///
/// Carries no secrets - just whether the runtime is configured/enabled, the
/// resolved binary, whether that binary is on PATH, and the bounded run limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterRuntimeStatus {
    pub plugin_id: String,
    /// The adapter plugin's display name (from its manifest).
    pub adapter_name: String,
    /// The recognized/configured runtime kind wire string, or `None` for an
    /// unconfigured, unrecognized adapter.
    pub kind: Option<String>,
    /// Whether a runtime config is persisted for this adapter.
    pub configured: bool,
    /// Whether the runtime is enabled.
    pub enabled: bool,
    /// The resolved binary that would be launched, if known.
    pub command: Option<String>,
    /// Whether that binary was found on the current PATH.
    pub available_on_path: bool,
    /// The resolved absolute path to the binary, if found.
    pub resolved_path: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub max_output_bytes: Option<u64>,
    pub working_dir: Option<String>,
    /// The coarse operational state, for at-a-glance status.
    pub state: AdapterRuntimeState,
    /// A human-readable, secret-free explanation of `state`.
    pub detail: String,
}

/// The coarse operational state of an adapter runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterRuntimeState {
    /// The local deterministic Prime adapter: always usable, no CLI, no config.
    LocalDeterministic,
    /// A CLI adapter that is enabled and whose binary is present on PATH.
    Available,
    /// Enabled, but the configured binary is not on PATH.
    MissingBinary,
    /// A runtime is configured but disabled.
    Disabled,
    /// A CLI-capable adapter with no enabled runtime yet (the safe default).
    NeedsConfiguration,
}

impl AdapterRuntimeState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AdapterRuntimeState::LocalDeterministic => "local_deterministic",
            AdapterRuntimeState::Available => "available",
            AdapterRuntimeState::MissingBinary => "missing_binary",
            AdapterRuntimeState::Disabled => "disabled",
            AdapterRuntimeState::NeedsConfiguration => "needs_configuration",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_known_adapters() {
        assert_eq!(
            recognize_adapter_kind(LOCAL_PRIME_ADAPTER_ID),
            Some(AdapterKind::LocalPrime)
        );
        assert_eq!(
            recognize_adapter_kind(CLAUDE_CLI_ADAPTER_ID),
            Some(AdapterKind::ClaudeCli)
        );
        assert_eq!(
            recognize_adapter_kind(CODEX_CLI_ADAPTER_ID),
            Some(AdapterKind::CodexCli)
        );
        assert_eq!(recognize_adapter_kind("relux-adapter-mystery"), None);
    }

    #[test]
    fn default_commands_match_kind() {
        assert_eq!(AdapterKind::ClaudeCli.default_command(), Some("claude"));
        assert_eq!(AdapterKind::CodexCli.default_command(), Some("codex"));
        assert_eq!(AdapterKind::Command.default_command(), None);
        assert_eq!(AdapterKind::LocalPrime.default_command(), None);
        assert!(AdapterKind::ClaudeCli.is_cli());
        assert!(!AdapterKind::LocalPrime.is_cli());
    }

    #[test]
    fn resolved_command_prefers_override_then_default() {
        let mut cfg = AdapterRuntimeConfig {
            plugin_id: CLAUDE_CLI_ADAPTER_ID.to_string(),
            kind: AdapterKind::ClaudeCli,
            enabled: true,
            command: None,
            timeout_seconds: 120,
            max_output_bytes: 1000,
            working_dir: None,
        };
        assert_eq!(cfg.resolved_command().as_deref(), Some("claude"));
        cfg.command = Some("/opt/claude".to_string());
        assert_eq!(cfg.resolved_command().as_deref(), Some("/opt/claude"));

        let generic = AdapterRuntimeConfig {
            plugin_id: "relux-adapter-x".to_string(),
            kind: AdapterKind::Command,
            enabled: true,
            command: None,
            timeout_seconds: 120,
            max_output_bytes: 1000,
            working_dir: None,
        };
        assert_eq!(generic.resolved_command(), None);
    }

    #[test]
    fn timeout_and_output_clamp() {
        assert_eq!(clamp_adapter_timeout(None), DEFAULT_ADAPTER_TIMEOUT_SECONDS);
        assert_eq!(clamp_adapter_timeout(Some(0)), MIN_ADAPTER_TIMEOUT_SECONDS);
        assert_eq!(
            clamp_adapter_timeout(Some(10_000_000)),
            MAX_ADAPTER_TIMEOUT_SECONDS
        );
        assert_eq!(clamp_adapter_timeout(Some(60)), 60);

        assert_eq!(
            clamp_adapter_max_output(None),
            DEFAULT_ADAPTER_MAX_OUTPUT_BYTES
        );
        assert_eq!(
            clamp_adapter_max_output(Some(1)),
            MIN_ADAPTER_MAX_OUTPUT_BYTES
        );
        assert_eq!(
            clamp_adapter_max_output(Some(u64::MAX)),
            MAX_ADAPTER_MAX_OUTPUT_BYTES
        );
    }

    #[test]
    fn config_serializes_without_secret_fields() {
        let cfg = AdapterRuntimeConfig {
            plugin_id: CLAUDE_CLI_ADAPTER_ID.to_string(),
            kind: AdapterKind::ClaudeCli,
            enabled: false,
            command: None,
            timeout_seconds: 120,
            max_output_bytes: 1_000_000,
            working_dir: None,
        };
        let v: serde_json::Value = serde_json::to_value(&cfg).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "command",
                "enabled",
                "kind",
                "max_output_bytes",
                "plugin_id",
                "timeout_seconds",
                "working_dir"
            ]
        );
        assert_eq!(v["kind"], "claude_cli");
        assert_eq!(v["enabled"], false);
    }
}
