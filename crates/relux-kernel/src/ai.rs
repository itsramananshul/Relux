//! Optional LLM-backed shaping of Prime's *conversational* replies.
//!
//! This is the first, deliberately small step toward the LLM-backed Prime the
//! product calls for (`docs/RELUX_MASTER_PLAN.md` section 2 "Prime is an
//! LLM-backed agent", section 8.1 `relux-adapter-openrouter`, section 17.1 "Prime
//! Must Be Smart And Grounded"). The MVP limitation note in section 22 - "Prime
//! is the deterministic, rule-based stand-in ... no LLM yet" - is what this
//! module begins to lift.
//!
//! ## The safety contract (binding)
//!
//! The LLM may shape *text only*. Every durable kernel state change - creating a
//! task, starting a run, installing a plugin, granting a permission, requesting
//! an approval - still comes exclusively from the deterministic
//! [`crate::KernelState::prime_turn`] plan/action path. This module never touches
//! the kernel, never mutates state, and is only ever handed a `PrimeTurn` that
//! the kernel already decided and executed.
//!
//! Concretely:
//!
//! - **Not configured** (no key, or `RELUX_LLM_DISABLED`) -> everything stays
//!   exactly as today: the deterministic reply is returned verbatim
//!   ([`AiMode::Deterministic`]).
//! - **Actionful turn** (the kernel executed an action or queued an approval) ->
//!   the deterministic reply is kept verbatim and marked
//!   [`AiMode::DeterministicForAction`]. The LLM is never asked to narrate a real
//!   state change, so it can never overclaim one.
//! - **Conversational turn** (a read-only answer or a clarification - greetings,
//!   status, explanations, brainstorming, unknown chat) -> when OpenRouter is
//!   configured, the LLM rephrases the *already-grounded* deterministic reply
//!   into something natural ([`AiMode::Openrouter`]). If the call fails, it falls
//!   back to the deterministic reply with a safe, non-secret note.
//!
//! Nothing here ever logs, serializes, or returns the API key.
//!
//! It is shaped as a free function plus a plain config so it can later move
//! behind a `relux-adapter-openrouter` plugin without changing callers.

use std::path::Path;
use std::time::Duration;

use relux_core::{
    PrimeDisposition, PrimeIntent, PrimePolishedStep, PrimeProposal, PrimeProposalPolish, PrimeTurn,
};
use serde::{Deserialize, Serialize};

/// OpenRouter's OpenAI-compatible chat-completions endpoint.
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Default model when `RELUX_OPENROUTER_MODEL` is unset: a cheap, broadly
/// available general model. Override with the env var.
const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";

/// Default request timeout when `RELUX_LLM_TIMEOUT_MS` is unset.
const DEFAULT_TIMEOUT_MS: u64 = 15_000;
/// Clamp bounds for the request timeout, so a stray env value can't make Prime
/// hang forever or time out instantly.
const MIN_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 120_000;

/// Upper bound on completion tokens we ask the provider for - conversational
/// replies are short, and this bounds cost/latency.
const MAX_TOKENS: u32 = 500;
/// Hard cap on the characters we accept back, so a runaway response can't bloat
/// the API payload regardless of what the provider returns.
const MAX_REPLY_CHARS: usize = 4_000;

// Bounds on a proposal-polish overlay. Presentation strings only, so these are
// generous-but-finite: a runaway model reply can never balloon the card.
/// Max characters kept for a polished one-line summary.
const MAX_POLISH_SUMMARY_CHARS: usize = 240;
/// Max characters kept for a single polished step title.
const MAX_POLISH_TITLE_CHARS: usize = 160;
/// Max characters kept for a single clarifying question / risk note.
const MAX_POLISH_NOTE_CHARS: usize = 240;
/// Max clarifying questions kept on the overlay.
const MAX_POLISH_QUESTIONS: usize = 4;
/// Max advisory risk notes kept on the overlay.
const MAX_POLISH_RISKS: usize = 4;

// --- Configuration ---------------------------------------------------------

/// Resolved AI configuration for one process.
///
/// The API key is held privately and is never part of any serialized surface -
/// see [`AiStatus`], which is the only thing this config exposes to the wire.
#[derive(Debug, Clone)]
pub struct AiConfig {
    /// The resolved plaintext API key, or `None`. Sourced (in precedence order)
    /// from the referenced secret in the local secret store, a legacy plaintext
    /// value in the config file, or `RELUX_OPENROUTER_API_KEY`. Private by
    /// construction: nothing serializes or logs this.
    api_key: Option<String>,
    /// The model id to request (resolved, never empty).
    pub model: String,
    /// `true` when `RELUX_LLM_DISABLED` forces deterministic mode.
    pub disabled: bool,
    /// Request timeout in milliseconds (already clamped to a sane range).
    pub timeout_ms: u64,
    /// The explicitly-selected Prime brain, or `None` for the legacy auto choice
    /// (OpenRouter when a key is present and not disabled, otherwise Local).
    pub brain: Option<PrimeBrain>,
    /// The NAME of the secret referenced for the API key, when the operator
    /// selected the key by reference (the preferred, write-only path). This is the
    /// secret's name only — never its value — kept so the status surface can show
    /// which secret is in use and whether it currently resolves.
    pub api_key_secret: Option<String>,
    /// `true` when [`AiConfig::api_key_secret`] names a secret that is NOT present
    /// in the secret store (so no usable key was resolved). Drives a clear
    /// "missing secret" status instead of a silent fall-through.
    pub secret_missing: bool,
}

impl AiConfig {
    /// Read configuration from the environment.
    ///
    /// Recognized variables:
    /// - `RELUX_OPENROUTER_API_KEY` - enables OpenRouter when non-empty.
    /// - `RELUX_OPENROUTER_MODEL` - model id (default [`DEFAULT_MODEL`]).
    /// - `RELUX_LLM_DISABLED` - any truthy value forces deterministic mode.
    /// - `RELUX_LLM_TIMEOUT_MS` - request timeout (default [`DEFAULT_TIMEOUT_MS`]).
    pub fn from_env() -> Self {
        let api_key = std::env::var("RELUX_OPENROUTER_API_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let model = std::env::var("RELUX_OPENROUTER_MODEL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let disabled = std::env::var("RELUX_LLM_DISABLED")
            .ok()
            .map(|v| is_truthy(&v))
            .unwrap_or(false);
        let timeout_ms = std::env::var("RELUX_LLM_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok());
        let brain = std::env::var("RELUX_PRIME_BRAIN")
            .ok()
            .and_then(|v| PrimeBrain::parse(&v));
        Self::from_parts(api_key, model, disabled, timeout_ms).with_brain(brain)
    }

    /// Build a config from already-read parts. Pure (no env access), so config
    /// resolution and defaults are unit-testable without touching process env.
    /// The brain defaults to `None` (legacy auto choice); set it with
    /// [`AiConfig::with_brain`].
    pub fn from_parts(
        api_key: Option<String>,
        model: Option<String>,
        disabled: bool,
        timeout_ms: Option<u64>,
    ) -> Self {
        let model = model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let timeout_ms = timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
        Self {
            api_key: api_key.filter(|k| !k.trim().is_empty()),
            model,
            disabled,
            timeout_ms,
            brain: None,
            api_key_secret: None,
            secret_missing: false,
        }
    }

    /// Set the explicit Prime brain (builder-style), returning `self`.
    pub fn with_brain(mut self, brain: Option<PrimeBrain>) -> Self {
        self.brain = brain;
        self
    }

    /// Whether the OpenRouter LLM path is actually live: a key is present AND not
    /// disabled. (Independent of which brain is selected.)
    pub fn enabled(&self) -> bool {
        self.api_key.is_some() && !self.disabled
    }

    /// The effective Prime brain: the explicit choice, or the legacy auto choice
    /// (OpenRouter when a key is configured and not disabled, else Local).
    pub fn effective_brain(&self) -> PrimeBrain {
        match self.brain {
            Some(b) => b,
            None => {
                if self.enabled() {
                    PrimeBrain::Openrouter
                } else {
                    PrimeBrain::Local
                }
            }
        }
    }

    /// Whether a key is configured at all (independent of the disabled flag).
    pub fn configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// Build the safe, key-free status surface for `GET /v1/relux/ai/status`.
    ///
    /// `mode` and `reason` reflect the *effective brain*. For the CLI brains the
    /// kernel layers in live adapter availability before this is returned (the
    /// status here only knows the selection, not whether the binary is on PATH).
    pub fn status(&self) -> AiStatus {
        self.status_for(self.effective_brain(), false)
    }

    /// Build the status surface for an explicitly-resolved brain.
    ///
    /// Unlike [`AiConfig::status`] (which reports `effective_brain`), this takes the
    /// brain the kernel actually resolved for the turn — including a CLI brain that
    /// was auto-adopted because its adapter is enabled and on PATH ([`resolve_brain`]).
    /// `auto_detected` drives an honest "auto-detected" explanation and the matching
    /// status flag so the dashboard never claims the operator selected it.
    pub fn status_for(&self, brain: PrimeBrain, auto_detected: bool) -> AiStatus {
        let mode = match brain {
            PrimeBrain::Local => AiMode::Deterministic,
            PrimeBrain::Openrouter => {
                if self.enabled() {
                    AiMode::Openrouter
                } else {
                    AiMode::Deterministic
                }
            }
            PrimeBrain::ClaudeCli => AiMode::ClaudeCli,
            PrimeBrain::CodexCli => AiMode::CodexCli,
        };
        let reason = match brain {
            PrimeBrain::Openrouter if self.enabled() => match &self.api_key_secret {
                Some(name) => format!(
                    "OpenRouter configured; key from secret '{name}'. Conversational replies use {}. Actions stay deterministic and kernel-grounded.",
                    self.model
                ),
                None => format!(
                    "OpenRouter configured; conversational replies use {}. Actions stay deterministic and kernel-grounded.",
                    self.model
                ),
            },
            PrimeBrain::Openrouter if self.configured() && self.disabled => {
                "An OpenRouter key is set but RELUX_LLM_DISABLED forces deterministic Prime."
                    .to_string()
            }
            PrimeBrain::Openrouter if self.secret_missing => match &self.api_key_secret {
                Some(name) => format!(
                    "OpenRouter brain selected but the referenced secret '{name}' is not set; add it under Secrets and Prime activates the key automatically. Until then Prime stays deterministic."
                ),
                None => "OpenRouter brain selected but its API key secret is missing; Prime stays deterministic until you set it.".to_string(),
            },
            PrimeBrain::Openrouter => {
                "OpenRouter brain selected but no API key is configured; Prime stays deterministic until you add one (set a secret, then reference it here)."
                    .to_string()
            }
            PrimeBrain::ClaudeCli => {
                "Claude CLI brain selected; conversational replies are delegated to the local `claude` CLI when its adapter is enabled. Actions stay deterministic and kernel-grounded."
                    .to_string()
            }
            PrimeBrain::CodexCli => {
                "Codex CLI brain selected; conversational replies are delegated to the local `codex` CLI when its adapter is enabled. Actions stay deterministic and kernel-grounded."
                    .to_string()
            }
            PrimeBrain::Local => {
                "Local brain: Prime runs fully deterministic and grounded in control-plane state.".to_string()
            }
        };
        // When the brain was auto-adopted (no explicit choice, no OpenRouter key, but
        // an enabled CLI adapter is on PATH), say so plainly instead of implying the
        // operator picked it. Falls through to the selection reason for every other case.
        let reason = if auto_detected {
            match brain {
                PrimeBrain::ClaudeCli => "Claude CLI brain auto-detected — its adapter is enabled and the `claude` binary is on PATH, so Prime answers through it. Pick a brain explicitly on Crew → Adapters to override. Actions stay deterministic and kernel-grounded.".to_string(),
                PrimeBrain::CodexCli => "Codex CLI brain auto-detected — its adapter is enabled and the `codex` binary is on PATH, so Prime answers through it. Pick a brain explicitly on Crew → Adapters to override. Actions stay deterministic and kernel-grounded.".to_string(),
                _ => reason,
            }
        } else {
            reason
        };
        AiStatus {
            mode,
            brain: brain.as_str().to_string(),
            configured: self.configured(),
            disabled: self.disabled,
            model: self.model.clone(),
            timeout_ms: self.timeout_ms,
            api_key_secret: self.api_key_secret.clone(),
            secret_missing: self.secret_missing,
            reason,
            auto_detected,
        }
    }

    /// Resolve the effective config from the local dashboard-written secrets file
    /// (when present) with environment fallback.
    ///
    /// This is the first-release product path: an operator configures Prime's
    /// OpenRouter key from the dashboard (no env vars). The file lives under the
    /// local data root and is gitignored. A value present in the file wins for
    /// that field; any field the file omits falls back to the environment, so the
    /// existing CLI-only `RELUX_OPENROUTER_*` setup keeps working. The key is held
    /// privately and is never serialized back out (see [`AiStatus`]).
    pub fn resolve(path: Option<&Path>) -> Self {
        Self::resolve_with(path, |name| {
            crate::secret_store::secret_store().resolve(name)
        })
    }

    /// Resolve the effective config like [`AiConfig::resolve`], but with an
    /// injected secret resolver so the key-by-reference path is unit-testable
    /// without the process-global secret store.
    ///
    /// Key-source precedence (highest first):
    /// 1. A referenced secret (`StoredAiConfig::api_key_secret`) resolved through
    ///    `resolve_secret`. If the named secret is absent/blank the key is treated
    ///    as missing — `secret_missing` is set and no key is used (no silent
    ///    fall-through to a stale plaintext value).
    /// 2. A legacy plaintext key stored in the config file (`api_key`).
    /// 3. The `RELUX_OPENROUTER_API_KEY` environment value.
    ///
    /// The resolved plaintext is held privately and is never serialized; only the
    /// secret NAME (never its value) is carried on the config for the status
    /// surface.
    pub fn resolve_with(
        path: Option<&Path>,
        resolve_secret: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let env = Self::from_env();
        let Some(path) = path else {
            return env;
        };
        let Some(stored) = read_stored_config(path) else {
            return env;
        };
        let model = stored
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or(env.model);
        let disabled = stored.disabled.unwrap_or(env.disabled);
        let brain = stored
            .brain
            .as_deref()
            .and_then(PrimeBrain::parse)
            .or(env.brain);

        let secret_name = stored
            .api_key_secret
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let (api_key, api_key_secret, secret_missing) = match secret_name {
            Some(name) => match resolve_secret(&name) {
                Some(v) if !v.trim().is_empty() => (Some(v), Some(name), false),
                // Referenced but unset/blank: surface a clean missing-secret state
                // and use no key (never fall back to a stale plaintext value).
                _ => (None, Some(name), true),
            },
            None => {
                let plain = stored
                    .api_key
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())
                    .or(env.api_key);
                (plain, None, false)
            }
        };
        Self {
            api_key,
            model,
            disabled,
            timeout_ms: env.timeout_ms,
            brain,
            api_key_secret,
            secret_missing,
        }
    }
}

// --- Dashboard-configured secrets file -------------------------------------

/// The on-disk AI provider configuration the dashboard writes.
///
/// It lives under the local data root (next to `RELUX_DB`) and is gitignored.
/// It DOES hold the API key at rest so Prime can use it without environment
/// variables, but it is never returned over the API: only the key-free
/// [`AiStatus`] crosses the wire. Today only OpenRouter is supported; Claude and
/// Codex adapters authenticate through their own local CLI login, not a key here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredAiConfig {
    /// The provider id. Only `"openrouter"` is honored today; recorded for
    /// forward-compatibility and so the dashboard can show what is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// A legacy plaintext OpenRouter API key. Present only when an older config
    /// stored one directly; the dashboard no longer writes this — it references a
    /// secret instead (see [`StoredAiConfig::api_key_secret`]). Kept for backward
    /// compatibility and the env/CLI path; mutually exclusive with `api_key_secret`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// The NAME of a secret in the local secret store that holds the API key. This
    /// is the preferred, write-only path: the config (and every API response)
    /// stores only the reference, never the value — the plaintext is resolved from
    /// the store at request time. Mutually exclusive with `api_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// The selected Prime brain (`local` | `openrouter` | `claude_cli` |
    /// `codex_cli`). Omitted means the legacy auto choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brain: Option<String>,
}

/// Read the stored AI config from `path`, or `None` when it is absent/unreadable
/// or does not parse. Never panics; never logs the key.
pub fn read_stored_config(path: &Path) -> Option<StoredAiConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist (merge) the dashboard-configured AI settings to `path`.
///
/// Each `Some` field is applied over any existing file so a partial update keeps
/// the rest; passing `api_key: Some("")` / `api_key_secret: Some("")` (or
/// whitespace) clears that source without disturbing the model/disabled flags.
/// The plaintext `api_key` and the `api_key_secret` reference are mutually
/// exclusive: setting one (to a non-empty value) clears the other so there is a
/// single source of truth for the key. Parent directories are created. On Unix
/// the file is written `0600`. Returns an io error on failure; never logs the key.
pub fn write_stored_config(
    path: &Path,
    provider: Option<String>,
    api_key: Option<String>,
    api_key_secret: Option<String>,
    model: Option<String>,
    disabled: Option<bool>,
    brain: Option<String>,
) -> std::io::Result<()> {
    let mut current = read_stored_config(path).unwrap_or_default();
    if let Some(p) = provider {
        let p = p.trim().to_string();
        current.provider = if p.is_empty() { None } else { Some(p) };
    }
    if let Some(k) = api_key {
        let k = k.trim().to_string();
        if k.is_empty() {
            current.api_key = None;
        } else {
            // A plaintext key supersedes any secret reference — keep one source of
            // truth so a stale reference can't shadow the value just set.
            current.api_key = Some(k);
            current.api_key_secret = None;
        }
    }
    if let Some(s) = api_key_secret {
        let s = s.trim().to_string();
        if s.is_empty() {
            current.api_key_secret = None;
        } else {
            // A secret reference supersedes any stored plaintext key (the secure
            // path wins): the value lives only in the write-only secret store.
            current.api_key_secret = Some(s);
            current.api_key = None;
        }
    }
    if let Some(m) = model {
        let m = m.trim().to_string();
        current.model = if m.is_empty() { None } else { Some(m) };
    }
    if let Some(d) = disabled {
        current.disabled = Some(d);
    }
    if let Some(b) = brain {
        let b = b.trim().to_string();
        // Normalize to the canonical wire string; an empty/unknown value clears
        // the selection (back to the legacy auto choice).
        current.brain = PrimeBrain::parse(&b).map(|pb| pb.as_str().to_string());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(&current).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Remove the stored AI config file entirely (a no-op when it is absent).
pub fn clear_stored_config(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// `true` for the usual truthy env spellings; anything else is false.
fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

// --- Public surfaces -------------------------------------------------------

/// Which path produced a Prime reply. Serializes to snake_case for the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiMode {
    /// The LLM path is off (no key or disabled); deterministic reply verbatim.
    Deterministic,
    /// The LLM is configured, but this turn changed state / awaits approval, so
    /// the deterministic reply was kept verbatim and the LLM was not consulted.
    DeterministicForAction,
    /// The reply text was shaped by the OpenRouter model.
    Openrouter,
    /// The reply text came from the local Claude CLI adapter (`claude -p`).
    ClaudeCli,
    /// The reply text came from the local Codex CLI adapter (`codex exec`).
    CodexCli,
}

/// Which provider Prime uses for its *conversational* replies (its "brain").
///
/// The operator may select one explicitly (`docs/RELUX_MASTER_PLAN.md` section 8.1
/// — adapter plugins are how Relux connects to a model/agent runtime). When no
/// brain is selected, [`resolve_brain`] picks one: a configured OpenRouter key, or
/// otherwise an enabled CLI adapter that is on PATH (auto-adoption, so Prime is a
/// real conversational agent out of the box per §10.1/§14), falling back to `Local`.
/// The brain never affects durable actions: every state change still comes from the
/// deterministic kernel path regardless of the brain. `Local` is the
/// always-available grounded stand-in; `Openrouter` uses the configured API key;
/// `ClaudeCli`/`CodexCli` delegate to a local coding-agent CLI the operator has
/// installed and enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimeBrain {
    /// The deterministic, grounded local operator. No external call.
    Local,
    /// OpenRouter (an API key configured in the dashboard or env).
    Openrouter,
    /// The local Claude CLI adapter (`relux-adapter-claude-cli`).
    ClaudeCli,
    /// The local Codex CLI adapter (`relux-adapter-codex-cli`).
    CodexCli,
}

impl PrimeBrain {
    /// The stable wire string for this brain.
    pub fn as_str(&self) -> &'static str {
        match self {
            PrimeBrain::Local => "local",
            PrimeBrain::Openrouter => "openrouter",
            PrimeBrain::ClaudeCli => "claude_cli",
            PrimeBrain::CodexCli => "codex_cli",
        }
    }

    /// Parse a wire string into a brain, accepting a few friendly spellings.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "local" | "deterministic" => Some(PrimeBrain::Local),
            "openrouter" => Some(PrimeBrain::Openrouter),
            "claude_cli" | "claude" => Some(PrimeBrain::ClaudeCli),
            "codex_cli" | "codex" => Some(PrimeBrain::CodexCli),
            _ => None,
        }
    }
}

/// How Prime's effective conversational brain was chosen for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainResolution {
    /// The operator explicitly selected this brain (`AiConfig.brain` is set) —
    /// including an explicit `Local`.
    Explicit,
    /// No explicit choice; a usable OpenRouter key is configured, so OpenRouter is
    /// used (the legacy auto choice).
    OpenRouterAuto,
    /// No explicit choice and no key; an enabled CLI adapter that is on PATH was
    /// auto-adopted so Prime is a real conversational agent out of the box.
    CliAutoDetected,
    /// No brain available; the deterministic local fallback.
    LocalFallback,
}

/// The CLI brains whose adapter is enabled AND whose binary resolved on PATH
/// (`AdapterRuntimeState::Available`), in PREFERENCE order (Claude, then Codex).
///
/// Pure: it only reads the snapshot the kernel already builds
/// (`KernelState::adapter_runtime_status`). The order is the auto-adoption
/// preference consumed by [`resolve_brain`].
pub fn available_cli_brains(statuses: &[relux_core::AdapterRuntimeStatus]) -> Vec<PrimeBrain> {
    let mut out = Vec::new();
    for (id, brain) in [
        (relux_core::CLAUDE_CLI_ADAPTER_ID, PrimeBrain::ClaudeCli),
        (relux_core::CODEX_CLI_ADAPTER_ID, PrimeBrain::CodexCli),
    ] {
        let available = statuses.iter().any(|s| {
            s.plugin_id == id && s.state == relux_core::AdapterRuntimeState::Available
        });
        if available {
            out.push(brain);
        }
    }
    out
}

/// Resolve Prime's effective conversational brain for a turn.
///
/// This is the doc-conformant ordering from `docs/RELUX_MASTER_PLAN.md` §10.1 (the
/// LLM brain is the PRIMARY surface; the deterministic classifier is a fallback
/// rail) and §14 (Claude / Codex CLI is the recommended first brain):
///
/// 1. An explicit operator choice always wins — including an explicit `Local`, so
///    an operator who deliberately wants the deterministic brain keeps it.
/// 2. No explicit choice but a usable OpenRouter key → OpenRouter (legacy auto).
/// 3. No explicit choice and no key → auto-adopt the first AVAILABLE CLI brain
///    (`available_clis`, most-preferred first), so a user who installed and ENABLED
///    a coding-agent CLI gets a real conversational Prime instead of templates.
/// 4. Nothing available → the deterministic local fallback.
///
/// Auto-adoption only fires for an adapter the operator already ENABLED (CLI
/// adapters are disabled by default), so it never spawns an external process the
/// operator did not opt into, and it changes NO durable-action path — every state
/// change still flows through the deterministic kernel path regardless of the brain.
/// Pure: no env, no network, no clock.
pub fn resolve_brain(
    cfg: &AiConfig,
    available_clis: &[PrimeBrain],
) -> (PrimeBrain, BrainResolution) {
    if let Some(b) = cfg.brain {
        return (b, BrainResolution::Explicit);
    }
    if cfg.enabled() {
        return (PrimeBrain::Openrouter, BrainResolution::OpenRouterAuto);
    }
    if let Some(&b) = available_clis
        .iter()
        .find(|b| matches!(b, PrimeBrain::ClaudeCli | PrimeBrain::CodexCli))
    {
        return (b, BrainResolution::CliAutoDetected);
    }
    (PrimeBrain::Local, BrainResolution::LocalFallback)
}

/// The safe, serializable AI status. Deliberately carries NO key material.
#[derive(Debug, Clone, Serialize)]
pub struct AiStatus {
    pub mode: AiMode,
    /// The selected Prime brain wire string (`local` | `openrouter` |
    /// `claude_cli` | `codex_cli`).
    pub brain: String,
    /// Whether a usable API key is present (never the key itself). `false` when a
    /// referenced secret is missing (see [`AiStatus::secret_missing`]).
    pub configured: bool,
    pub disabled: bool,
    pub model: String,
    pub timeout_ms: u64,
    /// The NAME of the secret the API key is referenced from (never the value), or
    /// `null` when the key is configured another way / not configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_secret: Option<String>,
    /// `true` when [`AiStatus::api_key_secret`] names a secret that is not set in
    /// the secret store, so no key was resolved. The dashboard shows a clear
    /// "set this secret" prompt rather than a silent deterministic fallback.
    pub secret_missing: bool,
    /// A human-readable, secret-free explanation of the current mode.
    pub reason: String,
    /// `true` when this brain was NOT an explicit operator choice but was
    /// auto-adopted because its CLI adapter is enabled and on PATH (see
    /// [`resolve_brain`]). Presentation only — the dashboard shows an
    /// "auto-detected" hint so the operator knows why Prime is using it.
    #[serde(default)]
    pub auto_detected: bool,
}

// --- Brain probe ------------------------------------------------------------

/// The coarse outcome of a safe brain probe (`POST /v1/relux/ai/probe`).
/// Serializes to snake_case for the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrainProbeStatus {
    /// The brain is usable right now.
    Ready,
    /// The adapter / key exists but is switched off.
    Disabled,
    /// A CLI brain is enabled but its binary is not on PATH.
    MissingBinary,
    /// A CLI brain's adapter is not enabled at all.
    NotConfigured,
    /// OpenRouter is selected but no usable API key is configured.
    MissingKey,
    /// The probe ran but the brain reported a failure (non-zero exit / timeout).
    Failed,
}

/// The result of a safe, read-only brain probe.
///
/// A probe never runs an agent turn, never bypasses permissions, and (for
/// OpenRouter) never sends a billable request — it is a *liveness and
/// configuration* check the dashboard can show as a clear status. For the CLI
/// brains it runs `<bin> --version` ([`crate::adapter::probe_cli_version`]); for
/// OpenRouter it reports whether the key resolves; for Local it is always ready.
#[derive(Debug, Clone, Serialize)]
pub struct BrainProbe {
    /// The probed brain's wire string.
    pub brain: String,
    /// Whether the brain is usable right now.
    pub ok: bool,
    /// The coarse status driving the dashboard badge.
    pub status: BrainProbeStatus,
    /// A human-readable, secret-free explanation + the next step on failure.
    pub detail: String,
    /// The CLI version line, when a version probe captured one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// How the probe was checked: `always_available` (Local), `config_only`
    /// (OpenRouter key resolution / CLI not yet runnable), or `version_probe`
    /// (a real `--version` spawn).
    pub checked: &'static str,
}

/// Probe the always-available Local deterministic brain.
pub fn probe_local() -> BrainProbe {
    BrainProbe {
        brain: PrimeBrain::Local.as_str().to_string(),
        ok: true,
        status: BrainProbeStatus::Ready,
        detail: "Local deterministic brain — always available, no external call.".to_string(),
        version: None,
        checked: "always_available",
    }
}

/// Probe the OpenRouter brain by resolving its configuration only.
///
/// This deliberately makes NO network request (so a probe never bills or leaks):
/// it reports whether a usable key resolves, whether the LLM path is disabled, or
/// whether the referenced secret is missing — with the exact next step.
pub fn probe_openrouter(cfg: &AiConfig) -> BrainProbe {
    let brain = PrimeBrain::Openrouter.as_str().to_string();
    if cfg.enabled() {
        return BrainProbe {
            brain,
            ok: true,
            status: BrainProbeStatus::Ready,
            detail: format!(
                "OpenRouter key resolves and Prime is enabled (model {}). Configuration check only — no request was sent.",
                cfg.model
            ),
            version: None,
            checked: "config_only",
        };
    }
    if cfg.disabled && cfg.configured() {
        return BrainProbe {
            brain,
            ok: false,
            status: BrainProbeStatus::Disabled,
            detail: "An OpenRouter key is set but Prime's LLM path is disabled (RELUX_LLM_DISABLED or the disabled toggle). Re-enable it to use OpenRouter.".to_string(),
            version: None,
            checked: "config_only",
        };
    }
    let detail = if cfg.secret_missing {
        match &cfg.api_key_secret {
            Some(name) => format!(
                "OpenRouter selected but the referenced secret '{name}' is not set. Add it under Secrets and re-probe."
            ),
            None => "OpenRouter selected but its API key secret is missing. Set it to activate the key.".to_string(),
        }
    } else {
        "No OpenRouter API key is configured. Set a secret, then reference it here.".to_string()
    };
    BrainProbe {
        brain,
        ok: false,
        status: BrainProbeStatus::MissingKey,
        detail,
        version: None,
        checked: "config_only",
    }
}

/// Classify a CLI brain probe from its live adapter status and an optional
/// `--version` outcome. Pure: the caller runs the (blocking) version probe only
/// when the adapter is `Available`, then hands the result here for the verdict.
pub fn classify_cli_probe(
    brain: PrimeBrain,
    adapter: Option<&relux_core::AdapterRuntimeStatus>,
    version: Option<crate::adapter::CliVersionProbe>,
) -> BrainProbe {
    let brain_str = brain.as_str().to_string();
    let bin = adapter
        .and_then(|a| a.command.clone())
        .unwrap_or_else(|| match brain {
            PrimeBrain::CodexCli => "codex".to_string(),
            _ => "claude".to_string(),
        });
    let state = adapter.map(|a| &a.state);
    match state {
        None | Some(relux_core::AdapterRuntimeState::NeedsConfiguration) => BrainProbe {
            brain: brain_str,
            ok: false,
            status: BrainProbeStatus::NotConfigured,
            detail: format!(
                "The {bin} adapter is not enabled yet. Click \"Use … for Prime\" (or Enable adapter), then re-test."
            ),
            version: None,
            checked: "config_only",
        },
        Some(relux_core::AdapterRuntimeState::Disabled) => BrainProbe {
            brain: brain_str,
            ok: false,
            status: BrainProbeStatus::Disabled,
            detail: format!("The {bin} adapter is configured but disabled. Enable it to use it."),
            version: None,
            checked: "config_only",
        },
        Some(relux_core::AdapterRuntimeState::MissingBinary) => BrainProbe {
            brain: brain_str,
            ok: false,
            status: BrainProbeStatus::MissingBinary,
            detail: format!(
                "`{bin}` was not found on PATH. Install the CLI and sign in, then Refresh and re-test."
            ),
            version: None,
            checked: "config_only",
        },
        // Defensive: a CLI adapter never reports the local-deterministic state.
        Some(relux_core::AdapterRuntimeState::LocalDeterministic) => probe_local(),
        Some(relux_core::AdapterRuntimeState::Available) => match version {
            Some(v) if v.ran && v.ok => BrainProbe {
                brain: brain_str,
                ok: true,
                status: BrainProbeStatus::Ready,
                detail: format!("{} Sign-in is verified on your first chat turn.", v.detail),
                version: v.version,
                checked: "version_probe",
            },
            Some(v) if v.ran => BrainProbe {
                brain: brain_str,
                ok: false,
                status: BrainProbeStatus::Failed,
                detail: v.detail,
                version: v.version,
                checked: "version_probe",
            },
            // Spawn failed despite an Available snapshot (e.g. removed from PATH
            // between snapshot and probe): report it honestly as missing.
            Some(v) => BrainProbe {
                brain: brain_str,
                ok: false,
                status: BrainProbeStatus::MissingBinary,
                detail: v.detail,
                version: None,
                checked: "version_probe",
            },
            None => BrainProbe {
                brain: brain_str,
                ok: true,
                status: BrainProbeStatus::Ready,
                detail: format!("`{bin}` is enabled and on PATH."),
                version: None,
                checked: "config_only",
            },
        },
    }
}

// --- Live chat probe --------------------------------------------------------
//
// The quick probe above proves *availability*: a CLI binary runs `--version`, an
// OpenRouter key *resolves*. It cannot prove Prime can actually complete a chat
// turn — for the CLI brains "sign-in is verified on your first chat turn", and
// OpenRouter is never contacted at all. The *live* probe closes that gap with an
// explicit, operator-triggered test that sends ONE tiny bounded prompt through
// the selected brain and reports a classified result.
//
// Safety contract (binding, same spirit as the quick probe):
// - It is NEVER run automatically — only on a deliberate operator click.
// - CLI brains run the SAME safe adapter invocation a real turn uses
//   (`build_adapter_args`, NO bypass/danger flag), with a bounded timeout +
//   output cap; the reply is redacted + truncated before it leaves the kernel.
// - OpenRouter makes one tiny, low-token (billable) request via the existing
//   client path; no key ever appears in the result.
// - Local is deterministic and labelled a fallback/test brain.
// - It creates NO task and NO run, and grants no broader permission. It is a
//   setup diagnostic only.
//
// Reference-driven: Hermes validates a provider with a real minimal completion
// and classifies the failure (auth / payment / timeout) rather than trusting a
// config check (`reference/hermes-agent-main/agent/auxiliary_client.py`
// `_mark_provider_unhealthy` / `_is_payment_error`). This mirrors that
// "prove it with a real call, then classify" shape onto the Relux brains.

/// A tiny, fixed prompt for the live chat probe. Asks for one short fixed token so
/// a success is unambiguous and the reply stays small.
const LIVE_PROBE_PROMPT: &str = "Reply with exactly this text and nothing else: relux probe ok";
/// Wall-clock bound on a live CLI chat turn — generous enough for a real cold turn
/// (the CLI may spin up), but never unbounded.
const LIVE_PROBE_CLI_TIMEOUT_MS: u64 = 60_000;
/// Output cap for a live CLI probe. A probe reply is tiny; this only stops a
/// runaway adapter from streaming forever.
const LIVE_PROBE_MAX_OUTPUT_BYTES: usize = 16 * 1024;
/// Completion-token cap for a live OpenRouter probe — keeps the billable call as
/// small as possible.
const LIVE_PROBE_MAX_TOKENS: u32 = 32;
/// Max characters kept from the sample reply shown back to the operator.
const LIVE_PROBE_SAMPLE_CHARS: usize = 280;

/// The coarse outcome of an explicit LIVE chat probe
/// (`POST /v1/relux/ai/probe/live`). Serializes to snake_case for the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveProbeStatus {
    /// A real chat turn completed and returned a usable reply.
    Ready,
    /// The brain cannot run a live turn yet (CLI adapter not enabled / not on
    /// PATH / disabled). The `detail` carries the exact next step.
    NotConfigured,
    /// OpenRouter is selected but no usable API key resolved.
    MissingKey,
    /// The provider/CLI reported an authentication / sign-in failure.
    AuthFailed,
    /// The brain did not respond before the bounded timeout.
    Timeout,
    /// The probe ran but the turn failed (non-zero exit / error envelope / no
    /// readable reply).
    Failed,
    /// A live probe is not implemented for this brain.
    Unsupported,
}

/// The result of an explicit live chat probe.
///
/// Unlike [`BrainProbe`] (availability only), this is produced by actually
/// completing one bounded chat turn. The `sample` is a redacted, truncated slice
/// of the real reply so the operator can see the brain answered.
#[derive(Debug, Clone, Serialize)]
pub struct LiveBrainProbe {
    /// The probed brain's wire string.
    pub brain: String,
    /// Whether the brain completed a usable chat turn.
    pub ok: bool,
    /// The coarse status driving the dashboard badge.
    pub status: LiveProbeStatus,
    /// A human-readable, secret-free explanation + the next step on failure.
    pub detail: String,
    /// A redacted, truncated slice of the real reply, when one came back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// How long the probe took, in milliseconds (0 when no call was made).
    pub duration_ms: u64,
    /// How the probe was checked: `local_fallback` (Local), `openrouter_chat`, or
    /// `cli_chat`.
    pub checked: &'static str,
}

impl LiveBrainProbe {
    /// A failed live probe with no sample (used for join panics / spawn errors).
    fn failed(brain: PrimeBrain, status: LiveProbeStatus, detail: String, duration_ms: u64, checked: &'static str) -> Self {
        Self {
            brain: brain.as_str().to_string(),
            ok: false,
            status,
            detail,
            sample: None,
            duration_ms,
            checked,
        }
    }
}

/// Redact + truncate a candidate reply into a small operator-facing sample.
/// Returns `None` for an empty reply (so callers can treat "no text" as a failure
/// rather than a fake success).
fn sample_reply(text: &str) -> Option<String> {
    let redacted = relux_core::redact_secrets(text);
    let trimmed = redacted.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars(trimmed, LIVE_PROBE_SAMPLE_CHARS))
}

/// Probe the always-available Local deterministic brain with a (deterministic)
/// live answer. No external provider is contacted and no usage is incurred.
pub fn probe_local_live() -> LiveBrainProbe {
    LiveBrainProbe {
        brain: PrimeBrain::Local.as_str().to_string(),
        ok: true,
        status: LiveProbeStatus::Ready,
        detail: "Local deterministic brain answered. This is the grounded fallback/test brain — no external provider was contacted and no usage was incurred.".to_string(),
        sample: Some("relux probe ok (local deterministic brain)".to_string()),
        duration_ms: 0,
        checked: "local_fallback",
    }
}

/// Build the minimal chat messages for a live probe.
fn build_live_probe_messages() -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: "system",
            content: "You are a connectivity probe. Reply with exactly the text the user asks for, and nothing else.".to_string(),
        },
        ChatMessage {
            role: "user",
            content: LIVE_PROBE_PROMPT.to_string(),
        },
    ]
}

/// Classify a live OpenRouter probe from the completion result. Pure: the caller
/// makes the (billable) request and hands the outcome here. `Ok(text)` is a real
/// reply; `Err(reason)` is the short, secret-free reason from
/// [`request_completion`].
pub fn classify_openrouter_live(result: Result<String, String>, duration_ms: u64) -> LiveBrainProbe {
    let brain = PrimeBrain::Openrouter.as_str().to_string();
    match result {
        Ok(text) => match sample_reply(&text) {
            Some(sample) => LiveBrainProbe {
                brain,
                ok: true,
                status: LiveProbeStatus::Ready,
                detail: "OpenRouter completed a live chat turn. The key works and Prime can reach the provider.".to_string(),
                sample: Some(sample),
                duration_ms,
                checked: "openrouter_chat",
            },
            None => LiveBrainProbe {
                brain,
                ok: false,
                status: LiveProbeStatus::Failed,
                detail: "OpenRouter returned an empty completion. The key may be valid but the model produced no text — try a different model, then re-test.".to_string(),
                sample: None,
                duration_ms,
                checked: "openrouter_chat",
            },
        },
        Err(reason) => {
            let (status, detail) = classify_openrouter_error(&reason);
            LiveBrainProbe {
                brain,
                ok: false,
                status,
                detail,
                sample: None,
                duration_ms,
                checked: "openrouter_chat",
            }
        }
    }
}

/// Map a secret-free OpenRouter failure reason to a coarse status + next step.
fn classify_openrouter_error(reason: &str) -> (LiveProbeStatus, String) {
    let r = reason.to_ascii_lowercase();
    if r.contains("no api key") {
        (
            LiveProbeStatus::MissingKey,
            "No OpenRouter API key resolved. Set a secret and reference it, then re-test.".to_string(),
        )
    } else if r.contains("401") || r.contains("403") {
        (
            LiveProbeStatus::AuthFailed,
            format!("OpenRouter rejected the request ({reason}). Check the key value and that the account is active, then re-test."),
        )
    } else if r.contains("timeout") {
        (
            LiveProbeStatus::Timeout,
            "OpenRouter did not respond before the timeout. Check connectivity (or raise the timeout), then re-test.".to_string(),
        )
    } else {
        (
            LiveProbeStatus::Failed,
            format!("OpenRouter live probe failed: {reason}."),
        )
    }
}

/// Run a live OpenRouter chat probe: one tiny, bounded, billable request, then a
/// classified [`LiveBrainProbe`]. When no usable key resolves (or the LLM path is
/// disabled) it returns WITHOUT making any request, so the probe never bills on a
/// misconfigured brain.
pub async fn probe_openrouter_live(cfg: &AiConfig) -> LiveBrainProbe {
    let brain = PrimeBrain::Openrouter.as_str().to_string();
    if cfg.disabled && cfg.configured() {
        return LiveBrainProbe {
            brain,
            ok: false,
            status: LiveProbeStatus::NotConfigured,
            detail: "An OpenRouter key is set but Prime's LLM path is disabled (RELUX_LLM_DISABLED or the disabled toggle). Re-enable it, then re-test.".to_string(),
            sample: None,
            duration_ms: 0,
            checked: "openrouter_chat",
        };
    }
    if !cfg.configured() {
        let detail = if cfg.secret_missing {
            match &cfg.api_key_secret {
                Some(name) => format!(
                    "OpenRouter selected but the referenced secret '{name}' is not set. Add it under Secrets, then re-test."
                ),
                None => "OpenRouter selected but its API key secret is missing. Set it to activate the key.".to_string(),
            }
        } else {
            "No OpenRouter API key is configured. Set a secret, then reference it here.".to_string()
        };
        return LiveBrainProbe {
            brain,
            ok: false,
            status: LiveProbeStatus::MissingKey,
            detail,
            sample: None,
            duration_ms: 0,
            checked: "openrouter_chat",
        };
    }
    let messages = build_live_probe_messages();
    let start = std::time::Instant::now();
    let result = request_completion_with(cfg, messages, LIVE_PROBE_MAX_TOKENS).await;
    let duration_ms = start.elapsed().as_millis() as u64;
    classify_openrouter_live(result, duration_ms)
}

/// Heuristic: does this (lowercased) CLI output look like an auth / sign-in
/// failure? Only consulted when the turn already FAILED, so a normal probe reply
/// (which says only "relux probe ok") never trips it.
fn looks_like_auth_failure(haystack_lower: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "not logged in",
        "please log in",
        "log in",
        "login",
        "sign in",
        "not signed in",
        "authentication",
        "unauthorized",
        "401",
        "403",
        "forbidden",
        "invalid api key",
        "api key",
        "credentials",
        "expired",
        "/login",
    ];
    NEEDLES.iter().any(|n| haystack_lower.contains(n))
}

/// Classify a live CLI chat probe from the process outcome + parsed summary. Pure:
/// the caller runs the safe adapter invocation (the SAME argv a real turn uses, NO
/// bypass/danger flag) and hands the outcome here for the verdict.
pub fn classify_cli_live_probe(
    brain: PrimeBrain,
    outcome: &crate::adapter::AdapterRunOutcome,
    summary: &relux_core::AdapterResultSummary,
    duration_ms: u64,
) -> LiveBrainProbe {
    let brain_str = brain.as_str().to_string();
    if outcome.timed_out {
        return LiveBrainProbe {
            brain: brain_str,
            ok: false,
            status: LiveProbeStatus::Timeout,
            detail: "The CLI did not finish a chat turn before the probe timeout. It may be waiting on input, downloading a model, or stuck. Run a chat turn manually to confirm, then re-test.".to_string(),
            sample: None,
            duration_ms,
            checked: "cli_chat",
        };
    }
    let process_ok = outcome.success && summary.is_error != Some(true);
    if process_ok {
        return match sample_reply(&summary.text) {
            Some(sample) => LiveBrainProbe {
                brain: brain_str,
                ok: true,
                status: LiveProbeStatus::Ready,
                detail: "The CLI completed a real chat turn and replied. Sign-in and the chat path are verified.".to_string(),
                sample: Some(sample),
                duration_ms,
                checked: "cli_chat",
            },
            // Exit 0 but no readable text — honest failure, never a fake success.
            None => LiveBrainProbe {
                brain: brain_str,
                ok: false,
                status: LiveProbeStatus::Failed,
                detail: "The CLI exited cleanly but produced no readable reply. It may not be fully signed in. Run a chat turn manually to confirm.".to_string(),
                sample: None,
                duration_ms,
                checked: "cli_chat",
            },
        };
    }
    let haystack = format!("{}\n{}\n{}", summary.text, outcome.stderr, outcome.stdout).to_ascii_lowercase();
    if looks_like_auth_failure(&haystack) {
        return LiveBrainProbe {
            brain: brain_str,
            ok: false,
            status: LiveProbeStatus::AuthFailed,
            detail: "The CLI reported an authentication / sign-in problem. Sign in to the CLI (run it once interactively to log in), then re-test.".to_string(),
            sample: None,
            duration_ms,
            checked: "cli_chat",
        };
    }
    let exit = outcome
        .exit_code
        .map(|c| format!(" (exit {c})"))
        .unwrap_or_default();
    LiveBrainProbe {
        brain: brain_str,
        ok: false,
        status: LiveProbeStatus::Failed,
        detail: format!("The CLI chat turn failed{exit}. Run a chat turn manually to see the full error, then re-test."),
        sample: None,
        duration_ms,
        checked: "cli_chat",
    }
}

/// Map a non-`Available` CLI adapter snapshot to a live-probe result WITHOUT
/// spawning anything (mirrors the quick probe's pre-spawn gating). Every
/// not-yet-runnable reason folds to [`LiveProbeStatus::NotConfigured`]; the
/// `detail` (reused from the quick probe) carries the specific next step.
pub fn cli_live_probe_unavailable(
    brain: PrimeBrain,
    adapter: Option<&relux_core::AdapterRuntimeStatus>,
) -> LiveBrainProbe {
    let quick = classify_cli_probe(brain, adapter, None);
    LiveBrainProbe {
        brain: quick.brain,
        ok: false,
        status: LiveProbeStatus::NotConfigured,
        detail: quick.detail,
        sample: None,
        duration_ms: 0,
        checked: "cli_chat",
    }
}

/// Run a live CLI chat probe end-to-end (BLOCKING — spawns a child process).
/// Resolves the binary, builds the SAME safe argv a real turn uses (no
/// bypass/danger flag), sends [`LIVE_PROBE_PROMPT`] on stdin under a bounded
/// timeout + output cap, then parses + classifies the result. Run this on a
/// blocking thread.
pub fn cli_live_probe_blocking(brain: PrimeBrain, bin: &str) -> LiveBrainProbe {
    let kind = match brain {
        PrimeBrain::CodexCli => relux_core::AdapterKind::CodexCli,
        _ => relux_core::AdapterKind::ClaudeCli,
    };
    let program = match crate::adapter::find_on_path(bin) {
        Some(p) => p.to_string_lossy().to_string(),
        None => {
            return LiveBrainProbe::failed(
                brain,
                LiveProbeStatus::NotConfigured,
                format!("`{bin}` is no longer on PATH. Install/enable the CLI and Refresh, then re-test."),
                0,
                "cli_chat",
            );
        }
    };
    let spec = crate::adapter::AdapterCommandSpec {
        program,
        args: crate::adapter::build_adapter_args(&kind),
        stdin: LIVE_PROBE_PROMPT.to_string(),
        working_dir: None,
        timeout: Duration::from_millis(LIVE_PROBE_CLI_TIMEOUT_MS),
        max_output_bytes: LIVE_PROBE_MAX_OUTPUT_BYTES,
    };
    let start = std::time::Instant::now();
    let outcome = crate::adapter::run_adapter_command(&spec);
    let duration_ms = start.elapsed().as_millis() as u64;
    match outcome {
        Ok(o) => {
            // `o.stdout`/`o.stderr` are already secret-redacted by the adapter.
            let summary = relux_core::parse_adapter_result(&o.stdout, kind);
            classify_cli_live_probe(brain, &o, &summary, duration_ms)
        }
        Err(e) => LiveBrainProbe::failed(
            brain,
            LiveProbeStatus::Failed,
            format!("Could not run `{bin}`: {e}."),
            duration_ms,
            "cli_chat",
        ),
    }
}

/// The result of (optionally) shaping a Prime reply.
#[derive(Debug, Clone)]
pub struct AiOutcome {
    pub mode: AiMode,
    /// The reply to return to the caller (LLM-shaped or deterministic).
    pub reply: String,
    /// The model used, set only when the LLM actually produced the reply.
    pub model: Option<String>,
    /// A safe, non-secret note - e.g. why the LLM was skipped or fell back.
    pub note: Option<String>,
}

impl AiOutcome {
    fn deterministic(reply: String) -> Self {
        Self {
            mode: AiMode::Deterministic,
            reply,
            model: None,
            note: None,
        }
    }
    /// A deterministic outcome that keeps the grounded reply but carries an
    /// optional, secret-free note explaining why a richer brain was not used
    /// (e.g. a CLI brain that is selected but unavailable). Public so the kernel's
    /// CLI-brain path can build it.
    pub fn deterministic_fallback(reply: String, note: Option<String>) -> Self {
        Self {
            mode: AiMode::Deterministic,
            reply,
            model: None,
            note,
        }
    }

    fn deterministic_for_action(reply: String) -> Self {
        Self {
            mode: AiMode::DeterministicForAction,
            reply,
            model: None,
            note: Some(
                "Action executed by the kernel; reply kept deterministic so no claim is invented."
                    .to_string(),
            ),
        }
    }
}

/// What this module decided to do with a turn, before any network call. Pure and
/// testable; the actual HTTP work happens only for [`AiPlan::Augment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiPlan {
    /// LLM off -> return the deterministic reply.
    Deterministic,
    /// LLM on but the turn was actionful -> keep the deterministic reply.
    DeterministicForAction,
    /// LLM on and the turn was conversational -> ask the model to rephrase.
    Augment,
}

/// A turn is "actionful" when the kernel changed durable state or queued an
/// approval. These replies are NEVER handed to the LLM - the LLM must not be in
/// a position to narrate (and possibly overclaim) a real state change.
pub fn is_actionful(turn: &PrimeTurn) -> bool {
    matches!(
        turn.disposition,
        PrimeDisposition::Executed | PrimeDisposition::AwaitingApproval
    ) || turn.created_task.is_some()
        || turn.started_run.is_some()
        || turn.approval.is_some()
        // Tool turns are grounded in real kernel output / a real refusal; the LLM
        // must never narrate (and possibly overclaim) a tool result or a tool
        // catalogue, so keep these deterministic too.
        || turn.invoked_tool.is_some()
        || turn.tool_output.is_some()
        || turn.tool_error.is_some()
        // A tool catalogue (ToolDiscovery) and a grounded multi-tool plan preview
        // (ToolPlanRequest) are built from the live tool registry; the LLM must never
        // re-narrate (and possibly overclaim) them, so keep their reply deterministic.
        || matches!(
            turn.intent,
            PrimeIntent::ToolDiscovery | PrimeIntent::ToolPlanRequest
        )
}

/// Decide the path for a turn given the config. Pure: no env, no network.
///
/// [`shape_reply`] handles only the text-based brains (Local and OpenRouter). For
/// the CLI brains the kernel intercepts the turn before calling this and spawns
/// the adapter itself, so here a CLI brain falls through to `Deterministic`
/// (which is also the correct fallback when a CLI turn is actionful).
pub fn plan_turn(cfg: &AiConfig, turn: &PrimeTurn) -> AiPlan {
    if is_actionful(turn) {
        return AiPlan::DeterministicForAction;
    }
    match cfg.effective_brain() {
        PrimeBrain::Openrouter if cfg.enabled() => AiPlan::Augment,
        // Local, OpenRouter-without-a-key, and the CLI brains (handled elsewhere)
        // all keep the grounded deterministic reply here.
        _ => AiPlan::Deterministic,
    }
}

/// Compose the single conversational prompt handed to a CLI brain on stdin
/// (`claude -p` / `codex exec`). It mirrors [`build_messages`]: it pins Prime's
/// identity and the hard "you did NOT perform any action" rule, supplies the
/// grounded deterministic reply as facts the CLI may rely on, and asks it to
/// answer the user naturally. Kept ASCII and self-contained so it works as a
/// one-shot prompt with no system-message channel.
pub fn compose_chat_prompt(message: &str, grounded_facts: &str) -> String {
    format!(
        "You are Prime, a general-purpose local AI agent — a helpful assistant and chat \
companion, like Codex or Hermes. You can also operate a local Relux control plane (tasks, \
runs, agents, plugins, permissions, approvals, an audit log) when the user asks for work, but \
conversation comes first. Speak naturally and concisely.\n\n\
Hard rules: you did NOT perform any action this turn, so never claim you created a task, \
started a run, installed a plugin, changed a permission, or modified any state. Answer like a \
normal assistant. Do NOT steer casual, emotional, or general conversation toward tasks, runs, \
or company setup, and do not mention the board/queue/crew unless the user asked about work or \
state; only when the user clearly wants work done should you mention what to say (for example: \
'create a task to summarize the README'). Do not invent runs, tasks, plugins, or numbers. Stay \
consistent with the grounded facts below. Use plain ASCII.\n\n\
Grounded control-plane facts you may rely on (do not contradict them, do not claim any \
action was performed):\n{grounded_facts}\n\nUser message:\n{message}\n\nReply to the user \
naturally."
    )
}

/// Shape one Prime reply, optionally via OpenRouter.
///
/// `message` is the user's original message; `turn` is the deterministic kernel
/// outcome (already executed). This never mutates kernel state and only ever
/// reads `turn.reply` as grounded facts for the model to rephrase.
pub async fn shape_reply(
    cfg: &AiConfig,
    message: &str,
    turn: &PrimeTurn,
    observations: &str,
) -> AiOutcome {
    match plan_turn(cfg, turn) {
        AiPlan::Deterministic => AiOutcome::deterministic(turn.reply.clone()),
        AiPlan::DeterministicForAction => AiOutcome::deterministic_for_action(turn.reply.clone()),
        AiPlan::Augment => {
            // Fold the read-only observations the governed tool loop gathered this turn (if
            // any) into the grounded facts the MODEL sees — never into the deterministic
            // fallback reply, which stays `turn.reply` on any failure.
            let facts = grounded_facts_with_observations(&turn.reply, observations);
            let messages = build_messages(message, &facts);
            let result = request_completion(cfg, messages).await;
            outcome_for_augment(cfg, turn.reply.clone(), result)
        }
    }
}

/// Fold the gathered read-only observations into the grounded facts handed to a reply-shaping
/// brain. With no observations the facts are exactly the deterministic reply (byte-for-byte the
/// prior behavior); with observations, the brain is told they are factual reads it performed this
/// turn so it can answer grounded in live state. Pure; never used as the fallback reply.
pub fn grounded_facts_with_observations(reply: &str, observations: &str) -> String {
    if observations.trim().is_empty() {
        reply.to_string()
    } else {
        format!(
            "{reply}\n\nLive read-only observations you gathered this turn (factual reads of the \
control plane; use them to answer, but you still performed NO action):\n{observations}"
        )
    }
}

/// Classify one user message into a structured intent via the OpenRouter brain.
///
/// Returns a validated [`crate::prime_intent::BrainIntentProposal`], or `None` on
/// ANY failure (no key, disabled, network error, or an unparseable reply) — every
/// failure path lands on the deterministic classifier, so the brain is strictly
/// additive. This is the structured LLM-mediated intent stage the master plan asks
/// for (§10.1, §17.1): the model only *proposes* the intent; the kernel still
/// validates it against the allowlist and reconciles it behind the fail-closed
/// safety gate. The raw model text is parsed by
/// [`crate::prime_intent::parse_intent_proposal`]; nothing un-validated escapes.
pub async fn classify_intent_via_openrouter(
    cfg: &AiConfig,
    message: &str,
) -> Option<crate::prime_intent::BrainIntentProposal> {
    if !cfg.enabled() || cfg.api_key.is_none() {
        return None;
    }
    let messages = vec![
        ChatMessage {
            role: "system",
            content: "You output only compact JSON. No prose, no code fences.".to_string(),
        },
        ChatMessage {
            role: "user",
            content: crate::prime_intent::build_intent_prompt(message),
        },
    ];
    match request_completion(cfg, messages).await {
        Ok(text) => crate::prime_intent::parse_intent_proposal(&text).ok(),
        Err(_) => None,
    }
}

/// Extract a task's structured slots from one user message via the OpenRouter
/// brain, as VALIDATED [`crate::prime_slots::BrainTaskSlots`], or `None` on ANY
/// failure (no key, disabled, network error, unparseable/unsupported reply).
///
/// This is the slot counterpart of [`classify_intent_via_openrouter`]: the model
/// only *proposes* the slots; the kernel still reconciles them against the
/// deterministic title and the live agent roster behind the fail-closed gate
/// ([`crate::prime_slots::reconcile_task_slots`]) before any task is created. The
/// raw model text is parsed by [`crate::prime_slots::parse_task_slots`]; nothing
/// un-validated escapes, and every failure lands on the deterministic slots so the
/// brain stays strictly additive (§10.1, §10.2, §17.1).
pub async fn extract_task_slots_via_openrouter(
    cfg: &AiConfig,
    message: &str,
) -> Option<crate::prime_slots::BrainTaskSlots> {
    if !cfg.enabled() || cfg.api_key.is_none() {
        return None;
    }
    let messages = vec![
        ChatMessage {
            role: "system",
            content: "You output only compact JSON. No prose, no code fences.".to_string(),
        },
        ChatMessage {
            role: "user",
            content: crate::prime_slots::build_task_slots_prompt(message),
        },
    ];
    match request_completion(cfg, messages).await {
        Ok(text) => crate::prime_slots::parse_task_slots(&text).ok(),
        Err(_) => None,
    }
}

/// Run one JSON-only extraction prompt through the OpenRouter brain, returning the raw
/// model text, or `None` on ANY failure (no key, disabled, network error). Shared by
/// the agent / plugin / permission slot extractors so each is a thin
/// "build prompt → complete → parse" wrapper, exactly like the task-slot path.
/// Run ONE round of the read-only context loop through the OpenRouter brain, returning the raw
/// model text (a tool-call JSON or a `{"done":true}` / final answer), or `None` on ANY failure
/// (no key, disabled, network error). The loop driver ([`crate::prime_tools::ContextLoop`])
/// interprets the text; this is just the per-round "prompt → text" primitive. The brain only ever
/// requests a READ-ONLY tool the kernel validates and executes — it changes nothing.
pub async fn complete_tool_round(cfg: &AiConfig, prompt: String) -> Option<String> {
    complete_json_only(cfg, prompt).await
}

/// Make one bounded, READ-ONLY diagnostic call: hand the brain a pre-built,
/// already-bounded + redacted diagnostic prompt
/// ([`crate::run_diagnosis::build_diagnostic_prompt`]) under a no-authority system
/// message and return its prose narrative, or `None` on ANY failure (no key,
/// disabled, network, empty). Unlike [`complete_json_only`] this asks for free
/// prose (the four-part diagnosis), not JSON. The caller assembles + re-bounds the
/// result (`crate::run_diagnosis::assemble`); this is just the "prompt → text"
/// primitive. The brain has no tools and changes nothing (§3.3b "diagnosis only").
pub async fn diagnose_via_openrouter(cfg: &AiConfig, prompt: String) -> Option<String> {
    if !cfg.enabled() || cfg.api_key.is_none() {
        return None;
    }
    let messages = vec![
        ChatMessage {
            role: "system",
            content: crate::run_diagnosis::DIAGNOSTIC_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user",
            content: prompt,
        },
    ];
    request_completion(cfg, messages).await.ok()
}

async fn complete_json_only(cfg: &AiConfig, prompt: String) -> Option<String> {
    if !cfg.enabled() || cfg.api_key.is_none() {
        return None;
    }
    let messages = vec![
        ChatMessage {
            role: "system",
            content: "You output only compact JSON. No prose, no code fences.".to_string(),
        },
        ChatMessage {
            role: "user",
            content: prompt,
        },
    ];
    request_completion(cfg, messages).await.ok()
}

/// Extract an agent's creation slots from one user message via the OpenRouter brain,
/// as VALIDATED [`crate::prime_agent_slots::BrainAgentSlots`], or `None` on ANY
/// failure. The kernel still reconciles them against the deterministic name and the
/// live agent/adapter rosters behind the fail-closed gate; the brain stays strictly
/// additive (§10.1, §10.2, §17.1).
pub async fn extract_agent_slots_via_openrouter(
    cfg: &AiConfig,
    message: &str,
) -> Option<crate::prime_agent_slots::BrainAgentSlots> {
    let text = complete_json_only(cfg, crate::prime_agent_slots::build_agent_slots_prompt(message))
        .await?;
    crate::prime_agent_slots::parse_agent_slots(&text).ok()
}

/// Extract an assignment's slots (`{task_id, agent_id}`) from one user message via the
/// OpenRouter brain, grounded in the live board, as VALIDATED
/// [`crate::prime_assign_slots::BrainAssignSlots`], or `None` on ANY failure. The kernel
/// still validates both ids against the live state before promoting any assignment; the
/// brain stays strictly additive (§10.1, §10.2, §17.1).
pub async fn extract_assign_slots_via_openrouter(
    cfg: &AiConfig,
    message: &str,
    summary: &relux_core::StateSummary,
) -> Option<crate::prime_assign_slots::BrainAssignSlots> {
    let text = complete_json_only(
        cfg,
        crate::prime_assign_slots::build_assign_slots_prompt(message, summary),
    )
    .await?;
    crate::prime_assign_slots::parse_assign_slots(&text).ok()
}

/// Extract a by-id task UPDATE's slots from one user message via the OpenRouter brain,
/// grounded in the live board, as VALIDATED [`crate::prime_update_slots::BrainUpdateSlots`],
/// or `None` on ANY failure. The kernel still validates the task/field/status/assignee
/// against the live state (and enforces the terminal-state guard) before applying
/// anything; the brain stays strictly additive (§10.1, §10.2, §17.1).
pub async fn extract_update_slots_via_openrouter(
    cfg: &AiConfig,
    message: &str,
    summary: &relux_core::StateSummary,
) -> Option<crate::prime_update_slots::BrainUpdateSlots> {
    let text = complete_json_only(
        cfg,
        crate::prime_update_slots::build_update_slots_prompt(message, summary),
    )
    .await?;
    crate::prime_update_slots::parse_update_slots(&text).ok()
}

/// Extract the plugin a user asked Prime to install via the OpenRouter brain, as a
/// VALIDATED [`crate::prime_admin_slots::BrainPluginRef`], or `None`. The install
/// stays approval-gated; this only sharpens the subject the human reviews.
pub async fn extract_plugin_ref_via_openrouter(
    cfg: &AiConfig,
    message: &str,
) -> Option<crate::prime_admin_slots::BrainPluginRef> {
    let text = complete_json_only(cfg, crate::prime_admin_slots::build_plugin_ref_prompt(message))
        .await?;
    crate::prime_admin_slots::parse_plugin_ref(&text).ok()
}

/// Extract the subject of a permission grant via the OpenRouter brain, as VALIDATED
/// [`crate::prime_admin_slots::BrainPermissionSlots`], or `None`. The grant stays
/// approval-gated; the kernel still validates the subject against the live agent
/// roster before proposing it.
pub async fn extract_permission_slots_via_openrouter(
    cfg: &AiConfig,
    message: &str,
) -> Option<crate::prime_admin_slots::BrainPermissionSlots> {
    let text = complete_json_only(
        cfg,
        crate::prime_admin_slots::build_permission_slots_prompt(message),
    )
    .await?;
    crate::prime_admin_slots::parse_permission_slots(&text).ok()
}

/// Re-word a clarify / brainstorm turn via the OpenRouter brain, returning the validated
/// polished text, or `None` on ANY failure (no key, disabled, network error, malformed
/// reply, a clarify that is not exactly one question, an action claim, low confidence, or
/// a pure echo). The brain only re-words a turn the kernel already decided is non-actionful;
/// it authors no action. Mirrors the slot extractors: build prompt → complete → parse →
/// reconcile, with everything un-validated dropped (§10.5, §17.1).
pub async fn polish_clarify_via_openrouter(
    cfg: &AiConfig,
    message: &str,
    deterministic_text: &str,
    kind: crate::prime_clarify::ClarifyKind,
) -> Option<String> {
    let text = complete_json_only(
        cfg,
        crate::prime_clarify::build_clarify_prompt(kind, message, deterministic_text),
    )
    .await?;
    let parsed = crate::prime_clarify::parse_clarify(&text, kind).ok()?;
    crate::prime_clarify::reconcile_clarify(deterministic_text, &parsed, kind)
}

/// Shape the POST-EXECUTION (after-action) reply for an actionful turn via the OpenRouter
/// brain, returning the validated wording, or `None` on ANY failure (no key, disabled, network
/// error, malformed reply, a contradiction of the real result, an invented id, low confidence,
/// or a pure echo).
///
/// The action has ALREADY been executed (or proposed) by the kernel; this only re-words the
/// confirmation, grounded ONLY in the sanitized [`crate::prime_after_action::ActionEnvelope`].
/// Mirrors the clarify path (build prompt → complete → parse → reconcile), with every claim
/// validated against the envelope so the brain can never narrate unexecuted work
/// (`docs/prime-processing-audit.md` "after-action narration", §10.2, §17.1).
pub async fn polish_after_action_via_openrouter(
    cfg: &AiConfig,
    message: &str,
    envelope: &crate::prime_after_action::ActionEnvelope,
) -> Option<String> {
    let text = complete_json_only(
        cfg,
        crate::prime_after_action::build_after_action_prompt(message, envelope),
    )
    .await?;
    let parsed = crate::prime_after_action::parse_after_action(&text, envelope).ok()?;
    crate::prime_after_action::reconcile_after_action(&envelope.grounded_reply, &parsed)
}

/// Produce ONE UNIFIED Prime decision (intent + every applicable slot + optional wording) in
/// a single OpenRouter call, as a [`crate::prime_decision::DecisionOutcome`]: a VALIDATED
/// [`crate::prime_decision::PrimeBrainDecision`], a `Malformed` reply (the provider answered but
/// [`crate::prime_decision::parse_decision`] rejected it — re-askable via the bounded
/// self-correction loop), or a `ProviderError` (no usable reply: no key / disabled / network /
/// empty envelope — NOT correctable). `correction` is empty on a normal round and carries the prior
/// round's validation error on a self-correction re-ask.
///
/// This is the one-shot counterpart to the separate `classify_intent_via_openrouter` +
/// `extract_*_slots_via_openrouter` + `polish_clarify_via_openrouter` calls: a configured
/// brain answers the whole turn at once, exactly as Hermes/Codex carry the answer and the
/// structured actions in one model response. The model only *proposes*; every section is
/// validated by its existing allowlist in [`crate::prime_decision::parse_decision`], and the
/// kernel still reconciles intent + slots against the live state behind the fail-closed gate.
/// On `None` the caller falls back to the specialized paths and the deterministic rails, so
/// the brain stays strictly additive (§10.1, §10.2, §17.1).
///
/// `observations` carries the rendered read-only reads the kernel already gathered earlier in this
/// turn's bounded observe-then-act loop ([`crate::prime_decision::DecisionLoop`]); it is empty on
/// the first round (so that prompt is byte-for-byte the prior single-shot prompt) and grounds the
/// brain's subsequent rounds in live state it asked to inspect.
pub async fn decide_prime_via_openrouter(
    cfg: &AiConfig,
    message: &str,
    summary: &relux_core::StateSummary,
    tools_inventory: &str,
    history: &str,
    observations: &str,
    correction: &str,
) -> crate::prime_decision::DecisionOutcome {
    use crate::prime_decision::DecisionOutcome;
    let Some(text) = complete_json_only(
        cfg,
        crate::prime_decision::build_decision_prompt_with_correction(
            message,
            summary,
            tools_inventory,
            history,
            observations,
            correction,
        ),
    )
    .await
    else {
        // No usable reply at all (no key / disabled / network error / empty envelope): not
        // correctable.
        return DecisionOutcome::ProviderError;
    };
    // The provider answered: a parse failure here is a malformed-but-correctable reply (re-askable),
    // distinct from the provider failure above. The error string IS the correction fed back.
    match crate::prime_decision::parse_decision(&text) {
        Ok(d) => DecisionOutcome::Decision(d),
        Err(e) => DecisionOutcome::Malformed(e),
    }
}

/// Combine an LLM result with the deterministic fallback into a final outcome.
/// Pure, so both the success and failure (fallback + note) paths are testable
/// without a network.
fn outcome_for_augment(
    cfg: &AiConfig,
    deterministic_reply: String,
    result: Result<String, String>,
) -> AiOutcome {
    match result {
        Ok(text) => AiOutcome {
            mode: AiMode::Openrouter,
            reply: text,
            model: Some(cfg.model.clone()),
            note: None,
        },
        Err(reason) => AiOutcome {
            mode: AiMode::Deterministic,
            reply: deterministic_reply,
            model: None,
            note: Some(format!("openrouter unavailable: {reason}")),
        },
    }
}

// --- Proposal polish (advisory, presentation-only) -------------------------
//
// The next rung of "LLM shapes text only": when the OpenRouter brain is enabled,
// it may also refine the WORDING of a plan-preview card — a clearer summary,
// per-step titles, clarifying questions, advisory risk notes. It has NO action
// authority: every authoritative field (step count, order, agent grounding,
// `multi_step`, and `goal`, which the commit re-wraps as `orchestrate <goal>`)
// comes only from the deterministic planner. A model suggestion is VALIDATED
// against the authoritative proposal before it is attached; anything that does not
// line up exactly is dropped and the deterministic preview stands. Nothing in the
// commit path ever reads the overlay (§10 planning layer, §11.1, §17.1).
//
// Two brains feed this validation chokepoint. OpenRouter goes through the HTTP
// path here ([`polish_proposal`]); the CLI brains (Claude / Codex) are spawned by
// the kernel with [`compose_polish_prompt`] on stdin and their reply is run
// through [`polish_from_cli_text`] — which lifts the JSON out of the adapter
// envelope and calls the SAME [`validate_polish`]. Whatever the prompt asks for,
// only titles/questions/risks/provenance can ever change; step count, order, and
// agent ids are immutable.

/// What to do with a proposal before any network call. Pure and testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolishPlan {
    /// Leave the deterministic proposal as-is (brain off / not OpenRouter / the
    /// proposal is single-step, so there is nothing to refine).
    Skip,
    /// OpenRouter is live and the proposal is a genuine multi-step plan: ask the
    /// model for a presentation overlay.
    Augment,
}

/// Whether a proposal carries anything worth polishing: a genuine multi-step plan
/// with at least one step. Single-step (or empty) proposals carry nothing to
/// refine and always skip, for every brain. Pure.
pub fn proposal_wants_polish(proposal: &PrimeProposal) -> bool {
    proposal.multi_step && !proposal.steps.is_empty()
}

/// Decide whether to polish a proposal over the OpenRouter HTTP path, given the
/// config. Pure: no env, no network. Restricted to the OpenRouter brain (the clean
/// JSON-returning path); the CLI brains get an equivalent polish via the kernel's
/// adapter spawn (see [`compose_polish_prompt`] / [`polish_from_cli_text`]), which
/// runs through the SAME [`validate_polish`] chokepoint. Single-step proposals
/// carry nothing to refine and always skip.
pub fn plan_polish(cfg: &AiConfig, proposal: &PrimeProposal) -> PolishPlan {
    if !proposal_wants_polish(proposal) {
        return PolishPlan::Skip;
    }
    match cfg.effective_brain() {
        PrimeBrain::Openrouter if cfg.enabled() => PolishPlan::Augment,
        _ => PolishPlan::Skip,
    }
}

/// The raw, untrusted polish a model returns. Every field is optional/defaulted so
/// a partial or malformed-but-parseable reply never panics; validation against the
/// authoritative proposal happens in [`validate_polish`].
#[derive(Debug, Clone, Default, Deserialize)]
struct PolishSuggestion {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    steps: Vec<PolishStepIn>,
    #[serde(default)]
    questions: Vec<String>,
    #[serde(default)]
    risks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PolishStepIn {
    /// 1-based index of the authoritative step this title refines. Defaults to 0
    /// when the model omits it, which matches no real step (steps are 1-based), so
    /// an unindexed suggestion is safely rejected rather than mis-applied.
    #[serde(default)]
    index: u32,
    #[serde(default)]
    title: String,
}

/// Validate a model suggestion against the AUTHORITATIVE proposal and distill the
/// advisory overlay. Pure: the proposal's steps/agents/goal are never mutated.
///
/// The invariant lives here: `step_titles` is accepted ONLY when the suggestion's
/// step indexes match the authoritative steps exactly — same count, same set, no
/// duplicates, no extras. Any mismatch (the model merged, split, reordered, added,
/// or renamed steps) drops the titles entirely and the deterministic titles stand.
/// When accepted, titles are emitted keyed to authoritative indexes in
/// authoritative order, so even a reordered model array yields a canonical overlay.
/// `questions`/`risks` are pure additive advisory text (trimmed, bounded). Returns
/// `None` when nothing usable survives validation.
fn validate_polish(proposal: &PrimeProposal, raw: PolishSuggestion) -> Option<PrimeProposalPolish> {
    let summary = clean_polish_text(raw.summary.as_deref().unwrap_or(""), MAX_POLISH_SUMMARY_CHARS);

    // Authoritative indexes, in order. The overlay can only ever speak about these.
    let authoritative: Vec<u32> = proposal.steps.iter().map(|s| s.index).collect();

    // Map the suggestion's titles by index, rejecting duplicates outright.
    let mut by_index: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    let mut duplicate = false;
    for s in &raw.steps {
        if by_index.insert(s.index, s.title.clone()).is_some() {
            duplicate = true;
        }
    }
    // Exact 1:1 correspondence with the authoritative steps, or no titles at all.
    let indexes_match = !duplicate
        && by_index.len() == authoritative.len()
        && authoritative.iter().all(|i| by_index.contains_key(i));
    let step_titles: Vec<PrimePolishedStep> = if indexes_match {
        authoritative
            .iter()
            .filter_map(|i| {
                let title = clean_polish_text(by_index.get(i).map(String::as_str).unwrap_or(""), MAX_POLISH_TITLE_CHARS);
                title.map(|title| PrimePolishedStep { index: *i, title })
            })
            .collect()
    } else {
        Vec::new()
    };

    let questions = clean_polish_notes(&raw.questions, MAX_POLISH_QUESTIONS);
    let risks = clean_polish_notes(&raw.risks, MAX_POLISH_RISKS);

    if summary.is_none() && step_titles.is_empty() && questions.is_empty() && risks.is_empty() {
        return None;
    }
    Some(PrimeProposalPolish {
        summary,
        step_titles,
        questions,
        risks,
        model: None,
    })
}

/// Trim, drop-if-empty, and truncate one presentation string.
fn clean_polish_text(s: &str, max: usize) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(truncate_chars(t, max))
    }
}

/// Trim/drop-empty/truncate a list of advisory notes and cap the count.
fn clean_polish_notes(notes: &[String], max_count: usize) -> Vec<String> {
    notes
        .iter()
        .filter_map(|n| clean_polish_text(n, MAX_POLISH_NOTE_CHARS))
        .take(max_count)
        .collect()
}

/// Combine an LLM polish result with the authoritative proposal into a final
/// overlay. Pure, so both the success and failure (no overlay) paths are testable
/// without a network. A failed call, or a suggestion that fails validation, yields
/// `None` — the caller then leaves the deterministic proposal unpolished.
fn finalize_polish(
    cfg: &AiConfig,
    proposal: &PrimeProposal,
    result: Result<PolishSuggestion, String>,
) -> Option<PrimeProposalPolish> {
    let suggestion = result.ok()?;
    let mut polish = validate_polish(proposal, suggestion)?;
    polish.model = Some(cfg.model.clone());
    Some(polish)
}

/// Validate a CLI brain's already-extracted reply text into an advisory polish
/// overlay, or `None`. This is the CLI counterpart of [`finalize_polish`]: it runs
/// the SAME chokepoint — [`parse_polish_json`] to lift the JSON object out of any
/// surrounding prose, then [`validate_polish`] to enforce that the suggestion can
/// never change the step count, order, or agent ids (only titles/questions/risks
/// survive) — and stamps `model` with the CLI's label for provenance on the card.
///
/// `text` is the human reply already lifted out of the adapter envelope by
/// [`relux_core::parse_adapter_result`] (the same shape seam the conversational
/// path uses). Malformed or prose-only text with no JSON object, or a suggestion
/// that fails validation, yields `None` and the deterministic preview stands.
/// Pure: never spawns, never mutates the proposal.
pub fn polish_from_cli_text(
    proposal: &PrimeProposal,
    text: &str,
    model_label: &str,
) -> Option<PrimeProposalPolish> {
    let suggestion = parse_polish_json(text).ok()?;
    let mut polish = validate_polish(proposal, suggestion)?;
    polish.model = Some(model_label.to_string());
    Some(polish)
}

/// Produce an advisory presentation overlay for a plan proposal, or `None`.
///
/// This NEVER mutates kernel state and NEVER changes what the plan does: it reads
/// the authoritative proposal as grounding, asks the model for better wording, and
/// returns a validated overlay (or `None` on skip/error/invalid). The caller
/// attaches it to `proposal.polish`; the commit path is unaffected.
pub async fn polish_proposal(
    cfg: &AiConfig,
    proposal: &PrimeProposal,
) -> Option<PrimeProposalPolish> {
    match plan_polish(cfg, proposal) {
        PolishPlan::Skip => None,
        PolishPlan::Augment => {
            let messages = build_polish_messages(proposal);
            let result = request_completion(cfg, messages)
                .await
                .and_then(|text| parse_polish_json(&text));
            finalize_polish(cfg, proposal, result)
        }
    }
}

/// Pull a `PolishSuggestion` out of a model reply. Tolerant of surrounding prose:
/// it lifts the first balanced-looking JSON object and parses that. Returns
/// `Err(reason)` (secret-free) when no usable JSON object is present.
fn parse_polish_json(text: &str) -> Result<PolishSuggestion, String> {
    let start = text.find('{').ok_or_else(|| "no json object".to_string())?;
    let end = text.rfind('}').ok_or_else(|| "no json object".to_string())?;
    if end <= start {
        return Err("no json object".to_string());
    }
    serde_json::from_str::<PolishSuggestion>(&text[start..=end])
        .map_err(|_| "invalid polish json".to_string())
}

/// Build the strict-JSON polish prompt. The system message forbids any structural
/// change and any action claim; the user message supplies the authoritative steps
/// (by index) the model must mirror exactly, and pins the output schema.
fn build_polish_messages(proposal: &PrimeProposal) -> Vec<ChatMessage> {
    const SYSTEM: &str = "You are Prime, refining the WORDING of an already-decided plan preview \
for a local Relux control plane. You have NO authority to change the plan. You MUST NOT add, \
remove, reorder, merge, or split steps; you MUST keep exactly one entry per given step, keyed by \
its index; you MUST NOT change which agent a step is assigned to; and you MUST NOT claim any work \
was created or run (a preview commits nothing). Improve only the wording: a clearer one-line \
summary, clearer step titles, a few clarifying questions, and advisory risk notes. Reply with a \
SINGLE JSON object and nothing else, using this schema: \
{\"summary\": string, \"steps\": [{\"index\": number, \"title\": string}], \"questions\": [string], \
\"risks\": [string]}. Use plain ASCII.";

    let steps_json: Vec<serde_json::Value> = proposal
        .steps
        .iter()
        .map(|s| {
            serde_json::json!({
                "index": s.index,
                "title": s.title,
                "role": s.role,
                "agent": s.agent,
            })
        })
        .collect();
    let grounding = serde_json::json!({
        "goal": proposal.goal,
        "steps": steps_json,
    });

    let user = format!(
        "Here is the authoritative plan preview to refine. Mirror its steps EXACTLY (same indexes, \
same count, same order, same agents); only improve the wording. Authoritative plan:\n{}\n\nReturn \
the JSON object described above.",
        serde_json::to_string_pretty(&grounding).unwrap_or_else(|_| "{}".to_string())
    );

    vec![
        ChatMessage {
            role: "system",
            content: SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user",
            content: user,
        },
    ]
}

/// Compose the single strict-JSON polish prompt handed to a CLI brain on stdin
/// (`claude -p` / `codex exec`). It mirrors [`build_polish_messages`] but folds the
/// system + user channels into one self-contained one-shot prompt (as
/// [`compose_chat_prompt`] does for the conversational path): it pins the "you may
/// refine WORDING only, never the plan" rule, supplies the authoritative steps (by
/// index) the model must mirror exactly, and pins the JSON-only output schema.
/// Kept ASCII so it works with no system-message channel.
///
/// The prompt is advisory, not load-bearing: whatever the CLI returns is still run
/// through [`polish_from_cli_text`] -> [`validate_polish`], so any structural drift
/// (added/dropped/reordered steps, a changed agent) is rejected regardless of what
/// the prompt asked for.
pub fn compose_polish_prompt(proposal: &PrimeProposal) -> String {
    let steps_json: Vec<serde_json::Value> = proposal
        .steps
        .iter()
        .map(|s| {
            serde_json::json!({
                "index": s.index,
                "title": s.title,
                "role": s.role,
                "agent": s.agent,
            })
        })
        .collect();
    let grounding = serde_json::json!({
        "goal": proposal.goal,
        "steps": steps_json,
    });

    format!(
        "You are Prime, refining the WORDING of an already-decided plan preview for a local \
Relux control plane. You have NO authority to change the plan. You MUST NOT add, remove, reorder, \
merge, or split steps; you MUST keep exactly one entry per given step, keyed by its index; you \
MUST NOT change which agent a step is assigned to; and you MUST NOT claim any work was created or \
run (a preview commits nothing). Improve only the wording: a clearer one-line summary, clearer \
step titles, a few clarifying questions, and advisory risk notes. Reply with a SINGLE JSON object \
and NOTHING else (no prose, no markdown, no code fences), using this schema: \
{{\"summary\": string, \"steps\": [{{\"index\": number, \"title\": string}}], \"questions\": [string], \
\"risks\": [string]}}. Use plain ASCII.\n\nAuthoritative plan to refine (mirror its steps EXACTLY \
- same indexes, same count, same order, same agents; only improve the wording):\n{}\n\nReturn the \
JSON object now.",
        serde_json::to_string_pretty(&grounding).unwrap_or_else(|_| "{}".to_string())
    )
}

// --- Prompt construction ---------------------------------------------------

/// Build the chat messages. The system prompt pins Prime's identity and the hard
/// rule that the model must not claim it performed any action; the deterministic
/// reply is supplied as grounded facts the model may rely on but must not
/// contradict.
fn build_messages(message: &str, grounded_facts: &str) -> Vec<ChatMessage> {
    const SYSTEM: &str = "You are Prime, a general-purpose local AI agent — a helpful \
assistant and chat companion, like Codex or Hermes. You can also operate a local Relux \
control plane (tasks, runs, agents, plugins, permissions, approvals, an audit log) when the \
user asks for work, but conversation comes first. Speak naturally and concisely. Hard rules: \
you did NOT perform any action this turn, so never claim you created a task, started a run, \
installed a plugin, changed a permission, or modified any state. Answer like a normal \
assistant. Do NOT steer casual, emotional, or general conversation toward tasks, runs, or \
company setup, and do not mention the board/queue/crew unless the user asked about work or \
state; only when the user clearly wants work done should you mention what to say (for example: \
'create a task to summarize the README'). Do not invent runs, tasks, plugins, or numbers. Stay \
consistent with the grounded facts you are given. Use plain ASCII.";

    let user = format!(
        "Grounded control-plane facts you may rely on (do not contradict them, do not claim \
any action was performed):\n{grounded_facts}\n\nUser message:\n{message}\n\nReply to the user \
naturally."
    );

    vec![
        ChatMessage {
            role: "system",
            content: SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user",
            content: user,
        },
    ]
}

// --- HTTP (OpenRouter) -----------------------------------------------------

#[derive(Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(serde::Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(serde::Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: ChatChoiceMessage,
}

#[derive(serde::Deserialize, Default)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

/// Make one bounded OpenRouter chat-completion call.
///
/// Returns `Ok(text)` on a usable reply, or `Err(reason)` with a short,
/// secret-free reason on any failure. The key travels only in the `Authorization`
/// header and never appears in an error.
async fn request_completion(cfg: &AiConfig, messages: Vec<ChatMessage>) -> Result<String, String> {
    request_completion_with(cfg, messages, MAX_TOKENS).await
}

/// Like [`request_completion`] but with an explicit completion-token cap, so a
/// caller that only needs a tiny reply (e.g. the live chat probe) can bound the
/// billable cost further than the default [`MAX_TOKENS`].
async fn request_completion_with(
    cfg: &AiConfig,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
) -> Result<String, String> {
    let key = cfg
        .api_key
        .as_deref()
        .ok_or_else(|| "no api key".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(cfg.timeout_ms))
        .build()
        .map_err(|_| "http client init failed".to_string())?;

    let body = ChatRequest {
        model: &cfg.model,
        messages,
        max_tokens,
        temperature: 0.4,
    };

    let resp = client
        .post(OPENROUTER_URL)
        .bearer_auth(key)
        // OpenRouter attribution headers (optional, non-secret).
        .header("X-Title", "Relux Prime")
        .header("HTTP-Referer", "https://github.com/itsramananshul/Relux")
        .json(&body)
        .send()
        .await
        .map_err(|e| classify_send_error(&e))?;

    if !resp.status().is_success() {
        // Status code only - response bodies can echo request content; keep the
        // note minimal and non-secret.
        return Err(format!("http {}", resp.status().as_u16()));
    }

    let parsed: ChatResponse = resp
        .json()
        .await
        .map_err(|_| "invalid response body".to_string())?;

    let text = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "empty completion".to_string())?;

    Ok(truncate_chars(&text, MAX_REPLY_CHARS))
}

/// Build the synchronous [`crate::mcp_sampling::Sampler`] an MCP managed-stdio session
/// uses to serve a gated `sampling/createMessage` request, or `None` when no usable
/// Prime/AI provider is configured (disabled, or no key) — in which case sampling
/// refuses cleanly with "no provider".
///
/// The key lives **only** on the in-memory [`AiConfig`] (resolved by secret reference)
/// and travels solely in the OpenRouter `Authorization` header inside
/// [`request_completion`]; it is **never** handed to the MCP server. Only the bounded,
/// redacted completion text crosses back. The output is bounded by the existing
/// [`MAX_TOKENS`] / [`MAX_REPLY_CHARS`] caps, then re-clamped + redacted by the sampling
/// handler before it leaves for the server.
pub fn build_sampling_sampler(cfg: &AiConfig) -> Option<crate::mcp_sampling::Sampler> {
    if !cfg.enabled() || cfg.api_key.is_none() {
        return None;
    }
    let cfg = cfg.clone();
    Some(std::sync::Arc::new(
        move |req: &crate::mcp_sampling::SamplingRequest| {
            let messages = build_sampling_chat_messages(req);
            run_blocking_completion(&cfg, messages).map(|text| {
                crate::mcp_sampling::SamplingCompletion {
                    text,
                    model: cfg.model.clone(),
                }
            })
        },
    ))
}

/// Map a bounded [`crate::mcp_sampling::SamplingRequest`] to OpenRouter chat messages.
fn build_sampling_chat_messages(req: &crate::mcp_sampling::SamplingRequest) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = Vec::new();
    if let Some(system) = &req.system {
        messages.push(ChatMessage {
            role: "system",
            content: system.clone(),
        });
    }
    for m in &req.messages {
        let role: &'static str = match m.role.as_str() {
            "assistant" => "assistant",
            "system" => "system",
            _ => "user",
        };
        messages.push(ChatMessage {
            role,
            content: m.text.clone(),
        });
    }
    messages
}

/// Run [`request_completion`] to completion **synchronously**, on a dedicated OS thread
/// with its own current-thread Tokio runtime. The MCP stdio pump is synchronous and may
/// already be inside the server's async runtime; a fresh thread is never a Tokio worker,
/// so `block_on` there is safe (it would panic on a worker thread). Bounded by the
/// provider's own request timeout. Returns a short, secret-free reason on failure.
fn run_blocking_completion(cfg: &AiConfig, messages: Vec<ChatMessage>) -> Result<String, String> {
    let cfg = cfg.clone();
    let handle = std::thread::spawn(move || -> Result<String, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "sampling runtime init failed".to_string())?;
        rt.block_on(request_completion(&cfg, messages))
    });
    handle
        .join()
        .map_err(|_| "sampling worker panicked".to_string())?
}

/// Map a reqwest send error to a short, stable, secret-free reason.
fn classify_send_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".to_string()
    } else if e.is_connect() {
        "connection failed".to_string()
    } else if e.is_request() {
        "request error".to_string()
    } else {
        "request failed".to_string()
    }
}

/// Truncate to at most `max` characters on a char boundary.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relux_core::TaskId;

    fn turn(disposition: PrimeDisposition, reply: &str) -> PrimeTurn {
        PrimeTurn {
            intent: PrimeIntent::Greeting,
            reply: reply.to_string(),
            disposition,
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
            tool_plan_proposal: None,
            pending_tool_approval: None,
            tool_trace: vec![],
        }
    }

    #[test]
    fn no_key_is_deterministic() {
        let cfg = AiConfig::from_parts(None, None, false, None);
        assert!(!cfg.enabled());
        assert!(!cfg.configured());
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(cfg.status().mode, AiMode::Deterministic);
    }

    #[test]
    fn key_enables_openrouter() {
        let cfg = AiConfig::from_parts(Some("test-key".into()), None, false, None);
        assert!(cfg.enabled());
        assert!(cfg.configured());
        assert_eq!(cfg.status().mode, AiMode::Openrouter);
    }

    #[test]
    fn disabled_forces_deterministic_even_with_key() {
        let cfg = AiConfig::from_parts(Some("test-key".into()), None, true, None);
        assert!(!cfg.enabled());
        assert!(cfg.configured(), "the key is still present");
        assert!(cfg.disabled);
        assert_eq!(cfg.status().mode, AiMode::Deterministic);
    }

    #[test]
    fn custom_model_and_clamped_timeout() {
        let cfg = AiConfig::from_parts(
            Some("k".into()),
            Some("anthropic/claude-3.5-haiku".into()),
            false,
            Some(10),
        );
        assert_eq!(cfg.model, "anthropic/claude-3.5-haiku");
        // 10ms is below the floor; clamped up to MIN_TIMEOUT_MS.
        assert_eq!(cfg.timeout_ms, MIN_TIMEOUT_MS);
        let cfg2 = AiConfig::from_parts(Some("k".into()), None, false, Some(10_000_000));
        assert_eq!(cfg2.timeout_ms, MAX_TIMEOUT_MS);
    }

    #[test]
    fn blank_strings_fall_back_to_defaults() {
        let cfg = AiConfig::from_parts(Some("   ".into()), Some("  ".into()), false, None);
        assert!(!cfg.configured(), "a blank key is treated as no key");
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn status_never_contains_the_key() {
        let secret = ["sk", "or", "v1", "THIS-MUST-NOT-LEAK"].join("-");
        let cfg = AiConfig::from_parts(Some(secret.clone()), None, false, None);
        let json = serde_json::to_string(&cfg.status()).unwrap();
        assert!(
            !json.contains(&secret),
            "status JSON must never carry the API key: {json}"
        );
        // It must still report configured=true and the safe fields.
        assert!(json.contains("\"configured\":true"));
        assert!(json.contains("\"mode\":\"openrouter\""));
    }

    #[test]
    fn status_json_has_only_safe_keys() {
        let cfg = AiConfig::from_parts(Some("secret".into()), None, false, None);
        let v: serde_json::Value = serde_json::to_value(cfg.status()).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "auto_detected",
                "brain",
                "configured",
                "disabled",
                "mode",
                "model",
                "reason",
                "secret_missing",
                "timeout_ms"
            ]
        );
        // No plaintext key under any name. `api_key_secret` is omitted entirely
        // here (this config used a plaintext key, not a reference).
        assert!(!obj.contains_key("api_key"));
        assert!(!obj.contains_key("api_key_secret"));
    }

    #[test]
    fn is_truthy_spellings() {
        for v in ["1", "true", "TRUE", "yes", "on", " On "] {
            assert!(is_truthy(v), "{v:?} should be truthy");
        }
        for v in ["0", "false", "no", "off", "", "maybe"] {
            assert!(!is_truthy(v), "{v:?} should be falsey");
        }
    }

    #[test]
    fn plan_is_deterministic_when_not_enabled() {
        let cfg = AiConfig::from_parts(None, None, false, None);
        let t = turn(PrimeDisposition::Answered, "hi");
        assert_eq!(plan_turn(&cfg, &t), AiPlan::Deterministic);
    }

    #[test]
    fn plan_augments_conversational_turns_when_enabled() {
        let cfg = AiConfig::from_parts(Some("k".into()), None, false, None);
        let answered = turn(PrimeDisposition::Answered, "There is 1 active run.");
        assert_eq!(plan_turn(&cfg, &answered), AiPlan::Augment);
        let clarify = turn(
            PrimeDisposition::NeedsClarification,
            "What should I create?",
        );
        assert_eq!(plan_turn(&cfg, &clarify), AiPlan::Augment);
    }

    #[test]
    fn plan_keeps_actionful_turns_deterministic() {
        let cfg = AiConfig::from_parts(Some("k".into()), None, false, None);
        let executed = turn(PrimeDisposition::Executed, "Created task_0001.");
        assert_eq!(plan_turn(&cfg, &executed), AiPlan::DeterministicForAction);

        let awaiting = turn(
            PrimeDisposition::AwaitingApproval,
            "I logged approval_0001.",
        );
        assert_eq!(plan_turn(&cfg, &awaiting), AiPlan::DeterministicForAction);

        // Even an "Answered" turn that carries a created artifact is actionful.
        let mut sneaky = turn(PrimeDisposition::Answered, "ok");
        sneaky.created_task = Some(TaskId::new("task_0009"));
        assert!(is_actionful(&sneaky));
        assert_eq!(plan_turn(&cfg, &sneaky), AiPlan::DeterministicForAction);
    }

    #[test]
    fn augment_success_is_openrouter() {
        let cfg = AiConfig::from_parts(Some("k".into()), Some("m/x".into()), false, None);
        let out = outcome_for_augment(&cfg, "fallback".into(), Ok("natural reply".into()));
        assert_eq!(out.mode, AiMode::Openrouter);
        assert_eq!(out.reply, "natural reply");
        assert_eq!(out.model.as_deref(), Some("m/x"));
        assert!(out.note.is_none());
    }

    #[test]
    fn augment_failure_falls_back_with_safe_note() {
        let cfg = AiConfig::from_parts(Some("k".into()), None, false, None);
        let out = outcome_for_augment(&cfg, "deterministic reply".into(), Err("timeout".into()));
        assert_eq!(out.mode, AiMode::Deterministic);
        assert_eq!(
            out.reply, "deterministic reply",
            "must fall back to the kernel's grounded reply"
        );
        assert!(out.model.is_none());
        assert_eq!(out.note.as_deref(), Some("openrouter unavailable: timeout"));
    }

    #[test]
    fn stored_config_round_trips_and_resolves_over_env() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ai-config.json");

        // No file yet -> resolve falls back to env (deterministic here: no key).
        let resolved = AiConfig::resolve(Some(&path));
        assert!(!resolved.configured(), "no file, no env key");

        // Write a key + model through the public writer.
        let secret = ["sk", "or", "stored", "DO-NOT-LEAK"].join("-");
        write_stored_config(
            &path,
            Some("openrouter".into()),
            Some(secret.clone()),
            None,
            Some("anthropic/claude-3.5-haiku".into()),
            None,
            None,
        )
        .unwrap();

        let resolved = AiConfig::resolve(Some(&path));
        assert!(resolved.configured(), "file key is picked up");
        assert!(resolved.enabled());
        assert_eq!(resolved.model, "anthropic/claude-3.5-haiku");
        // The status surface still never carries the key.
        let json = serde_json::to_string(&resolved.status()).unwrap();
        assert!(!json.contains(&secret), "status leaked the key: {json}");

        // A partial update keeps the key but flips disabled.
        write_stored_config(&path, None, None, None, None, Some(true), None).unwrap();
        let resolved = AiConfig::resolve(Some(&path));
        assert!(resolved.configured(), "key preserved across partial update");
        assert!(!resolved.enabled(), "disabled flag applied");

        // Clearing only the key (empty string) removes it.
        write_stored_config(&path, None, Some("   ".into()), None, None, None, None).unwrap();
        let resolved = AiConfig::resolve(Some(&path));
        assert!(!resolved.configured(), "blank key clears the stored key");

        // Clearing the file entirely returns to env fallback.
        clear_stored_config(&path).unwrap();
        assert!(!path.exists());
        assert!(!AiConfig::resolve(Some(&path)).configured());
    }

    #[test]
    fn provider_key_resolves_from_a_secret_reference_without_storing_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ai-config.json");

        // The dashboard path: store ONLY a reference to a named secret, never the
        // value. The config file must not contain the plaintext key.
        let secret_value = ["sk", "or", "v1", "REFERENCED-NEVER-IN-CONFIG"].join("-");
        write_stored_config(
            &path,
            Some("openrouter".into()),
            None,
            Some("openrouter_api_key".into()),
            None,
            None,
            Some("openrouter".into()),
        )
        .unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains(&secret_value),
            "config file must never carry the plaintext key: {on_disk}"
        );
        assert!(
            on_disk.contains("openrouter_api_key"),
            "config keeps the secret NAME reference: {on_disk}"
        );

        // Resolve with a store that HAS the secret: the key is usable, enabled,
        // and the status carries the secret name but never the value.
        let resolved = AiConfig::resolve_with(Some(&path), |name| {
            if name == "openrouter_api_key" {
                Some(secret_value.clone())
            } else {
                None
            }
        });
        assert!(resolved.configured(), "referenced secret resolves to a key");
        assert!(resolved.enabled());
        assert!(!resolved.secret_missing);
        assert_eq!(resolved.api_key_secret.as_deref(), Some("openrouter_api_key"));
        let json = serde_json::to_string(&resolved.status()).unwrap();
        assert!(
            !json.contains(&secret_value),
            "status leaked the resolved key: {json}"
        );
        assert!(json.contains("openrouter_api_key"), "status names the secret");
    }

    #[test]
    fn missing_referenced_secret_fails_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ai-config.json");
        write_stored_config(
            &path,
            Some("openrouter".into()),
            None,
            Some("absent_key".into()),
            None,
            None,
            Some("openrouter".into()),
        )
        .unwrap();

        // The store does not have the secret -> no usable key, a clean missing
        // state, never a leak, and Prime stays deterministic (not enabled).
        let resolved = AiConfig::resolve_with(Some(&path), |_| None);
        assert!(!resolved.configured(), "missing secret => no usable key");
        assert!(!resolved.enabled());
        assert!(resolved.secret_missing, "missing-secret state is surfaced");
        assert_eq!(resolved.api_key_secret.as_deref(), Some("absent_key"));
        let status = resolved.status();
        assert!(status.secret_missing);
        assert!(
            status.reason.contains("absent_key") && status.reason.contains("not set"),
            "reason should name the missing secret and what to do: {}",
            status.reason
        );
    }

    #[test]
    fn secret_reference_and_plaintext_key_are_mutually_exclusive() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ai-config.json");

        // Start with a legacy plaintext key.
        let legacy = ["sk", "or", "LEGACY-PLAINTEXT"].join("-");
        write_stored_config(&path, None, Some(legacy.clone()), None, None, None, None).unwrap();
        assert_eq!(
            read_stored_config(&path).unwrap().api_key.as_deref(),
            Some(legacy.as_str())
        );

        // Setting a secret reference clears the stored plaintext key.
        write_stored_config(&path, None, None, Some("ref_name".into()), None, None, None).unwrap();
        let stored = read_stored_config(&path).unwrap();
        assert_eq!(stored.api_key, None, "plaintext cleared by secret ref");
        assert_eq!(stored.api_key_secret.as_deref(), Some("ref_name"));

        // Setting a plaintext key again clears the reference.
        write_stored_config(&path, None, Some(legacy.clone()), None, None, None, None).unwrap();
        let stored = read_stored_config(&path).unwrap();
        assert_eq!(stored.api_key_secret, None, "ref cleared by plaintext key");
        assert_eq!(stored.api_key.as_deref(), Some(legacy.as_str()));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel");
        // Multi-byte chars must not panic on a non-boundary cut.
        assert_eq!(truncate_chars("aaa", 2).chars().count(), 2);
    }

    #[test]
    fn brain_parse_round_trips_and_accepts_aliases() {
        for b in [
            PrimeBrain::Local,
            PrimeBrain::Openrouter,
            PrimeBrain::ClaudeCli,
            PrimeBrain::CodexCli,
        ] {
            assert_eq!(PrimeBrain::parse(b.as_str()), Some(b));
        }
        assert_eq!(PrimeBrain::parse("claude"), Some(PrimeBrain::ClaudeCli));
        assert_eq!(PrimeBrain::parse("CLAUDE-CLI"), Some(PrimeBrain::ClaudeCli));
        assert_eq!(PrimeBrain::parse("codex"), Some(PrimeBrain::CodexCli));
        assert_eq!(PrimeBrain::parse("deterministic"), Some(PrimeBrain::Local));
        assert_eq!(PrimeBrain::parse("nonsense"), None);
    }

    #[test]
    fn effective_brain_defaults_to_auto_then_explicit_wins() {
        // No key, no explicit brain -> Local.
        let local = AiConfig::from_parts(None, None, false, None);
        assert_eq!(local.effective_brain(), PrimeBrain::Local);
        // Key present, no explicit brain -> OpenRouter (legacy auto).
        let auto = AiConfig::from_parts(Some("k".into()), None, false, None);
        assert_eq!(auto.effective_brain(), PrimeBrain::Openrouter);
        // Explicit Local wins even with a key present.
        let forced_local = auto.clone().with_brain(Some(PrimeBrain::Local));
        assert_eq!(forced_local.effective_brain(), PrimeBrain::Local);
        // Explicit Claude CLI is honored regardless of key.
        let claude = local.with_brain(Some(PrimeBrain::ClaudeCli));
        assert_eq!(claude.effective_brain(), PrimeBrain::ClaudeCli);
        assert_eq!(claude.status().brain, "claude_cli");
        assert_eq!(claude.status().mode, AiMode::ClaudeCli);
    }

    #[test]
    fn cli_brain_keeps_shape_reply_deterministic() {
        // shape_reply only handles text brains; a CLI brain falls through to the
        // grounded deterministic reply here (the kernel spawns the CLI itself).
        let cfg = AiConfig::from_parts(Some("k".into()), None, false, None)
            .with_brain(Some(PrimeBrain::ClaudeCli));
        let conversational = turn(PrimeDisposition::Answered, "There is 1 active run.");
        assert_eq!(plan_turn(&cfg, &conversational), AiPlan::Deterministic);
    }

    #[test]
    fn explicit_local_brain_ignores_present_key() {
        let cfg = AiConfig::from_parts(Some("k".into()), None, false, None)
            .with_brain(Some(PrimeBrain::Local));
        let conversational = turn(PrimeDisposition::Answered, "grounded.");
        // Even though a key is present, the operator chose Local: no OpenRouter.
        assert_eq!(plan_turn(&cfg, &conversational), AiPlan::Deterministic);
    }

    #[test]
    fn brain_persists_through_stored_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ai-config.json");
        write_stored_config(&path, None, None, None, None, None, Some("claude_cli".into()))
            .unwrap();
        let resolved = AiConfig::resolve(Some(&path));
        assert_eq!(resolved.effective_brain(), PrimeBrain::ClaudeCli);
        // An unknown brain string clears the selection (back to auto).
        write_stored_config(&path, None, None, None, None, None, Some("nope".into())).unwrap();
        let resolved = AiConfig::resolve(Some(&path));
        assert_eq!(resolved.brain, None);
    }

    /// Build an adapter runtime status row for the brain-resolution tests.
    fn adapter_status(id: &str, state: relux_core::AdapterRuntimeState) -> relux_core::AdapterRuntimeStatus {
        relux_core::AdapterRuntimeStatus {
            plugin_id: id.to_string(),
            adapter_name: id.to_string(),
            kind: None,
            configured: true,
            enabled: state != relux_core::AdapterRuntimeState::Disabled
                && state != relux_core::AdapterRuntimeState::NeedsConfiguration,
            command: None,
            available_on_path: state == relux_core::AdapterRuntimeState::Available,
            resolved_path: None,
            timeout_seconds: None,
            max_output_bytes: None,
            working_dir: None,
            state,
            detail: String::new(),
        }
    }

    #[test]
    fn available_cli_brains_lists_only_available_in_preference_order() {
        use relux_core::AdapterRuntimeState::*;
        // Both available -> Claude first (preference order), then Codex.
        let both = vec![
            adapter_status(relux_core::CODEX_CLI_ADAPTER_ID, Available),
            adapter_status(relux_core::CLAUDE_CLI_ADAPTER_ID, Available),
        ];
        assert_eq!(
            available_cli_brains(&both),
            vec![PrimeBrain::ClaudeCli, PrimeBrain::CodexCli]
        );
        // Enabled-but-not-on-PATH / disabled / unconfigured never count.
        let none_ready = vec![
            adapter_status(relux_core::CLAUDE_CLI_ADAPTER_ID, MissingBinary),
            adapter_status(relux_core::CODEX_CLI_ADAPTER_ID, Disabled),
        ];
        assert!(available_cli_brains(&none_ready).is_empty());
        // Only Codex available -> only Codex.
        let codex_only = vec![adapter_status(relux_core::CODEX_CLI_ADAPTER_ID, Available)];
        assert_eq!(available_cli_brains(&codex_only), vec![PrimeBrain::CodexCli]);
    }

    #[test]
    fn resolve_brain_auto_adopts_an_available_cli_when_unset() {
        // No explicit brain, no key, Claude available -> auto-adopt Claude.
        let cfg = AiConfig::from_parts(None, None, false, None);
        let (brain, res) = resolve_brain(&cfg, &[PrimeBrain::ClaudeCli, PrimeBrain::CodexCli]);
        assert_eq!(brain, PrimeBrain::ClaudeCli);
        assert_eq!(res, BrainResolution::CliAutoDetected);

        // Nothing available -> deterministic Local fallback.
        let (brain, res) = resolve_brain(&cfg, &[]);
        assert_eq!(brain, PrimeBrain::Local);
        assert_eq!(res, BrainResolution::LocalFallback);
    }

    #[test]
    fn resolve_brain_honors_explicit_choice_and_openrouter_key_first() {
        // An OpenRouter key (no explicit brain) wins over an available CLI.
        let keyed = AiConfig::from_parts(Some("k".into()), None, false, None);
        let (brain, res) = resolve_brain(&keyed, &[PrimeBrain::ClaudeCli]);
        assert_eq!(brain, PrimeBrain::Openrouter);
        assert_eq!(res, BrainResolution::OpenRouterAuto);

        // An EXPLICIT Local is never auto-overridden, even with a CLI available.
        let forced_local =
            AiConfig::from_parts(None, None, false, None).with_brain(Some(PrimeBrain::Local));
        let (brain, res) = resolve_brain(&forced_local, &[PrimeBrain::ClaudeCli]);
        assert_eq!(brain, PrimeBrain::Local);
        assert_eq!(res, BrainResolution::Explicit);

        // An explicit Codex choice is honored even if only Claude is "available".
        let forced_codex =
            AiConfig::from_parts(None, None, false, None).with_brain(Some(PrimeBrain::CodexCli));
        let (brain, res) = resolve_brain(&forced_codex, &[PrimeBrain::ClaudeCli]);
        assert_eq!(brain, PrimeBrain::CodexCli);
        assert_eq!(res, BrainResolution::Explicit);
    }

    #[test]
    fn status_for_marks_auto_detected_cli_brain() {
        let cfg = AiConfig::from_parts(None, None, false, None);
        let st = cfg.status_for(PrimeBrain::ClaudeCli, true);
        assert_eq!(st.brain, "claude_cli");
        assert_eq!(st.mode, AiMode::ClaudeCli);
        assert!(st.auto_detected);
        assert!(st.reason.contains("auto-detected"));
        // The plain status (explicit) never claims auto-detection.
        let plain = cfg.status_for(PrimeBrain::Local, false);
        assert!(!plain.auto_detected);
    }

    #[test]
    fn probe_local_is_always_ready() {
        let p = probe_local();
        assert_eq!(p.brain, "local");
        assert!(p.ok);
        assert_eq!(p.status, BrainProbeStatus::Ready);
        assert_eq!(p.checked, "always_available");
    }

    #[test]
    fn probe_openrouter_reports_each_config_state() {
        // Usable key, enabled -> ready (no network call: config_only).
        let keyed = AiConfig::from_parts(Some("k".into()), None, false, None);
        let p = probe_openrouter(&keyed);
        assert!(p.ok);
        assert_eq!(p.status, BrainProbeStatus::Ready);
        assert_eq!(p.checked, "config_only");

        // Key present but disabled -> disabled.
        let disabled = AiConfig::from_parts(Some("k".into()), None, true, None);
        let p = probe_openrouter(&disabled);
        assert!(!p.ok);
        assert_eq!(p.status, BrainProbeStatus::Disabled);

        // No key at all -> missing_key with a clear next step.
        let bare = AiConfig::from_parts(None, None, false, None);
        let p = probe_openrouter(&bare);
        assert!(!p.ok);
        assert_eq!(p.status, BrainProbeStatus::MissingKey);
        assert!(p.detail.to_lowercase().contains("secret"));
    }

    #[test]
    fn probe_openrouter_flags_a_missing_secret_reference() {
        // A referenced-but-unset secret resolves to no key and a missing flag.
        // Build that state directly so the test is hermetic (no secret store).
        let mut cfg = AiConfig::from_parts(None, None, false, None);
        cfg.api_key_secret = Some("openrouter-key".into());
        cfg.secret_missing = true;
        let p = probe_openrouter(&cfg);
        assert_eq!(p.status, BrainProbeStatus::MissingKey);
        assert!(p.detail.contains("openrouter-key"));
    }

    #[test]
    fn classify_cli_probe_maps_adapter_state_to_status() {
        use relux_core::AdapterRuntimeState::*;
        // Not enabled at all (no adapter status) -> not_configured.
        let p = classify_cli_probe(PrimeBrain::ClaudeCli, None, None);
        assert!(!p.ok);
        assert_eq!(p.status, BrainProbeStatus::NotConfigured);

        // Disabled adapter -> disabled.
        let st = adapter_status(relux_core::CLAUDE_CLI_ADAPTER_ID, Disabled);
        let p = classify_cli_probe(PrimeBrain::ClaudeCli, Some(&st), None);
        assert_eq!(p.status, BrainProbeStatus::Disabled);

        // Enabled but binary missing -> missing_binary.
        let st = adapter_status(relux_core::CODEX_CLI_ADAPTER_ID, MissingBinary);
        let p = classify_cli_probe(PrimeBrain::CodexCli, Some(&st), None);
        assert_eq!(p.status, BrainProbeStatus::MissingBinary);
    }

    #[test]
    fn classify_cli_probe_uses_version_outcome_when_available() {
        use relux_core::AdapterRuntimeState::Available;
        let st = adapter_status(relux_core::CLAUDE_CLI_ADAPTER_ID, Available);

        // A clean version probe -> ready, with the captured version surfaced.
        let ok = crate::adapter::CliVersionProbe {
            ran: true,
            ok: true,
            version: Some("claude 1.2.3".into()),
            detail: "`claude` is installed and runnable.".into(),
        };
        let p = classify_cli_probe(PrimeBrain::ClaudeCli, Some(&st), Some(ok));
        assert!(p.ok);
        assert_eq!(p.status, BrainProbeStatus::Ready);
        assert_eq!(p.version.as_deref(), Some("claude 1.2.3"));
        assert_eq!(p.checked, "version_probe");

        // A non-zero version probe -> failed (not ready), even though on PATH.
        let bad = crate::adapter::CliVersionProbe {
            ran: true,
            ok: false,
            version: None,
            detail: "`claude --version` exited with status 1.".into(),
        };
        let p = classify_cli_probe(PrimeBrain::ClaudeCli, Some(&st), Some(bad));
        assert!(!p.ok);
        assert_eq!(p.status, BrainProbeStatus::Failed);
    }

    // --- Live chat probe ---------------------------------------------------

    /// Build an [`crate::adapter::AdapterRunOutcome`] for a live-probe test.
    fn cli_outcome(
        success: bool,
        timed_out: bool,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
    ) -> crate::adapter::AdapterRunOutcome {
        crate::adapter::AdapterRunOutcome {
            program: "claude".to_string(),
            exit_code,
            success,
            timed_out,
            cancelled: false,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            stdout_truncated: false,
            stderr_truncated: false,
            duration_ms: 12,
        }
    }

    #[test]
    fn probe_local_live_is_always_ready_and_labelled_test() {
        let p = probe_local_live();
        assert_eq!(p.brain, "local");
        assert!(p.ok);
        assert_eq!(p.status, LiveProbeStatus::Ready);
        assert_eq!(p.checked, "local_fallback");
        // It must say plainly that no provider was contacted (no fake usage).
        assert!(p.detail.to_lowercase().contains("no external provider"));
        assert!(p.sample.is_some());
    }

    #[test]
    fn classify_openrouter_live_maps_each_outcome() {
        // A real reply -> ready, with a redacted/bounded sample.
        let p = classify_openrouter_live(Ok("relux probe ok".to_string()), 80);
        assert!(p.ok);
        assert_eq!(p.status, LiveProbeStatus::Ready);
        assert_eq!(p.sample.as_deref(), Some("relux probe ok"));
        assert_eq!(p.duration_ms, 80);
        assert_eq!(p.checked, "openrouter_chat");

        // An empty completion is an honest failure, not a fake success.
        let p = classify_openrouter_live(Ok("   ".to_string()), 5);
        assert!(!p.ok);
        assert_eq!(p.status, LiveProbeStatus::Failed);
        assert!(p.sample.is_none());

        // No key -> missing_key.
        let p = classify_openrouter_live(Err("no api key".to_string()), 0);
        assert_eq!(p.status, LiveProbeStatus::MissingKey);

        // A 401/403 -> auth_failed.
        let p = classify_openrouter_live(Err("http 401".to_string()), 30);
        assert_eq!(p.status, LiveProbeStatus::AuthFailed);

        // A timeout -> timeout.
        let p = classify_openrouter_live(Err("timeout".to_string()), 60);
        assert_eq!(p.status, LiveProbeStatus::Timeout);

        // Anything else -> failed (and the reason is surfaced, not swallowed).
        let p = classify_openrouter_live(Err("http 500".to_string()), 10);
        assert_eq!(p.status, LiveProbeStatus::Failed);
        assert!(p.detail.contains("http 500"));
    }

    #[tokio::test]
    async fn probe_openrouter_live_never_calls_without_a_usable_key() {
        // No key -> missing_key, no request made (duration stays 0).
        let bare = AiConfig::from_parts(None, None, false, None);
        let p = probe_openrouter_live(&bare).await;
        assert_eq!(p.status, LiveProbeStatus::MissingKey);
        assert_eq!(p.duration_ms, 0);
        assert!(p.sample.is_none());

        // Key present but disabled -> not_configured, still no request.
        let disabled = AiConfig::from_parts(Some("k".into()), None, true, None);
        let p = probe_openrouter_live(&disabled).await;
        assert_eq!(p.status, LiveProbeStatus::NotConfigured);
        assert_eq!(p.duration_ms, 0);
    }

    #[test]
    fn classify_cli_live_probe_succeeds_on_a_real_reply() {
        let summary = relux_core::parse_adapter_result(
            r#"{"type":"result","is_error":false,"result":"relux probe ok"}"#,
            relux_core::AdapterKind::ClaudeCli,
        );
        let outcome = cli_outcome(true, false, Some(0), "", "");
        let p = classify_cli_live_probe(PrimeBrain::ClaudeCli, &outcome, &summary, 900);
        assert!(p.ok);
        assert_eq!(p.status, LiveProbeStatus::Ready);
        assert_eq!(p.sample.as_deref(), Some("relux probe ok"));
        assert_eq!(p.checked, "cli_chat");
    }

    #[test]
    fn classify_cli_live_probe_exit_zero_without_text_is_failed() {
        let summary = relux_core::parse_adapter_result("", relux_core::AdapterKind::CodexCli);
        let outcome = cli_outcome(true, false, Some(0), "", "");
        let p = classify_cli_live_probe(PrimeBrain::CodexCli, &outcome, &summary, 50);
        assert!(!p.ok);
        assert_eq!(p.status, LiveProbeStatus::Failed);
        assert!(p.sample.is_none());
    }

    #[test]
    fn classify_cli_live_probe_detects_timeout_and_auth_failure() {
        // A timeout is reported as such regardless of the (empty) output.
        let summary = relux_core::parse_adapter_result("", relux_core::AdapterKind::ClaudeCli);
        let timed = cli_outcome(false, true, None, "", "");
        let p = classify_cli_live_probe(PrimeBrain::ClaudeCli, &timed, &summary, 60_000);
        assert_eq!(p.status, LiveProbeStatus::Timeout);

        // A failed turn whose stderr looks like a sign-in problem -> auth_failed.
        let summary = relux_core::parse_adapter_result("", relux_core::AdapterKind::ClaudeCli);
        let auth = cli_outcome(
            false,
            false,
            Some(1),
            "",
            "Error: Not logged in. Run `claude /login` to authenticate.",
        );
        let p = classify_cli_live_probe(PrimeBrain::ClaudeCli, &auth, &summary, 120);
        assert_eq!(p.status, LiveProbeStatus::AuthFailed);

        // A generic non-zero exit with no auth signal -> failed (exit surfaced).
        let summary = relux_core::parse_adapter_result("", relux_core::AdapterKind::ClaudeCli);
        let fail = cli_outcome(false, false, Some(2), "boom", "segfault");
        let p = classify_cli_live_probe(PrimeBrain::ClaudeCli, &fail, &summary, 30);
        assert_eq!(p.status, LiveProbeStatus::Failed);
        assert!(p.detail.contains("exit 2"));
    }

    #[test]
    fn cli_live_probe_unavailable_folds_to_not_configured_with_next_step() {
        use relux_core::AdapterRuntimeState::*;
        // Not enabled at all -> not_configured, no spawn.
        let p = cli_live_probe_unavailable(PrimeBrain::ClaudeCli, None);
        assert!(!p.ok);
        assert_eq!(p.status, LiveProbeStatus::NotConfigured);
        assert_eq!(p.checked, "cli_chat");
        // The quick-probe detail (the specific next step) is preserved.
        assert!(!p.detail.is_empty());

        // A missing binary also folds to not_configured (the detail explains it).
        let st = adapter_status(relux_core::CODEX_CLI_ADAPTER_ID, MissingBinary);
        let p = cli_live_probe_unavailable(PrimeBrain::CodexCli, Some(&st));
        assert_eq!(p.status, LiveProbeStatus::NotConfigured);
        assert!(p.detail.to_lowercase().contains("path"));
    }

    #[test]
    fn chat_prompt_carries_facts_and_no_action_rule() {
        let p = compose_chat_prompt("hey", "There is 1 active run.");
        assert!(p.contains("hey"));
        assert!(p.contains("There is 1 active run."));
        assert!(p.contains("did NOT perform any action"));
    }

    // --- Proposal polish (advisory, presentation-only) ---------------------

    fn step(index: u32, title: &str, role: &str, agent: &str) -> relux_core::PrimeProposalStep {
        relux_core::PrimeProposalStep {
            index,
            title: title.to_string(),
            role: role.to_string(),
            agent: agent.to_string(),
        }
    }

    fn multi_proposal() -> PrimeProposal {
        PrimeProposal {
            goal: "ship the beta".to_string(),
            multi_step: true,
            steps: vec![
                step(1, "research the options", "research", "research-agent"),
                step(2, "build a prototype", "implementation", "prime"),
            ],
            agents: vec!["research-agent".to_string(), "prime".to_string()],
            polish: None,
        }
    }

    fn polish_step(index: u32, title: &str) -> PolishStepIn {
        PolishStepIn {
            index,
            title: title.to_string(),
        }
    }

    #[test]
    fn plan_polish_skips_unless_openrouter_live_and_multi_step() {
        let p = multi_proposal();
        // No key -> Skip even for a multi-step plan.
        let no_key = AiConfig::from_parts(None, None, false, None);
        assert_eq!(plan_polish(&no_key, &p), PolishPlan::Skip);
        // OpenRouter live + multi-step -> Augment.
        let live = AiConfig::from_parts(Some("k".into()), None, false, None);
        assert_eq!(plan_polish(&live, &p), PolishPlan::Augment);
        // A CLI brain keeps the deterministic preview (no structured polish).
        let cli = live.clone().with_brain(Some(PrimeBrain::ClaudeCli));
        assert_eq!(plan_polish(&cli, &p), PolishPlan::Skip);
        // A single-step proposal carries nothing to refine -> Skip even when live.
        let single = PrimeProposal {
            multi_step: false,
            steps: vec![],
            agents: vec![],
            ..multi_proposal()
        };
        assert_eq!(plan_polish(&live, &single), PolishPlan::Skip);
    }

    #[test]
    fn validate_polish_applies_titles_only_on_exact_index_match() {
        let p = multi_proposal();
        // Exact 1:1 by index -> titles applied, keyed to authoritative indexes.
        let ok = PolishSuggestion {
            summary: Some("A clear two-stage path to a shippable beta.".into()),
            steps: vec![
                polish_step(2, "Build a working prototype"),
                polish_step(1, "Survey the available options"),
            ],
            questions: vec![],
            risks: vec![],
        };
        let overlay = validate_polish(&p, ok).expect("a valid suggestion produces an overlay");
        assert_eq!(overlay.step_titles.len(), 2);
        // Emitted in AUTHORITATIVE order regardless of the model's array order.
        assert_eq!(overlay.step_titles[0].index, 1);
        assert_eq!(overlay.step_titles[0].title, "Survey the available options");
        assert_eq!(overlay.step_titles[1].index, 2);
        assert_eq!(overlay.step_titles[1].title, "Build a working prototype");
        assert!(overlay.summary.is_some());
    }

    #[test]
    fn validate_polish_rejects_titles_that_change_count_order_or_agents() {
        let p = multi_proposal();

        // The model tried to ADD a third step -> all titles dropped (count differs).
        let extra = PolishSuggestion {
            steps: vec![
                polish_step(1, "a"),
                polish_step(2, "b"),
                polish_step(3, "c"),
            ],
            ..Default::default()
        };
        assert!(
            validate_polish(&p, extra).is_none(),
            "an added step must drop the whole overlay (no usable advisory left)"
        );

        // The model dropped a step -> titles dropped (set differs).
        let fewer = PolishSuggestion {
            steps: vec![polish_step(1, "only one")],
            ..Default::default()
        };
        assert!(validate_polish(&p, fewer).is_none());

        // The model renamed an index (1,3 instead of 1,2) -> titles dropped.
        let renamed = PolishSuggestion {
            steps: vec![polish_step(1, "a"), polish_step(3, "b")],
            ..Default::default()
        };
        assert!(validate_polish(&p, renamed).is_none());

        // A duplicate index is rejected too.
        let dup = PolishSuggestion {
            steps: vec![polish_step(1, "a"), polish_step(1, "b")],
            ..Default::default()
        };
        assert!(validate_polish(&p, dup).is_none());

        // When a mismatch coexists with a valid summary, the summary survives but
        // the step titles are still dropped: the authoritative titles stand.
        let mixed = PolishSuggestion {
            summary: Some("nice plan".into()),
            steps: vec![polish_step(9, "ghost step")],
            ..Default::default()
        };
        let overlay = validate_polish(&p, mixed).expect("the summary is still usable");
        assert!(
            overlay.step_titles.is_empty(),
            "mismatched step indexes must yield no polished titles"
        );
        assert_eq!(overlay.summary.as_deref(), Some("nice plan"));

        // The authoritative proposal is never mutated by validation.
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[0].agent, "research-agent");
        assert_eq!(p.steps[1].agent, "prime");
    }

    #[test]
    fn validate_polish_bounds_questions_and_risks() {
        let p = multi_proposal();
        let many = PolishSuggestion {
            questions: vec![
                "q1".into(),
                "  ".into(), // blank dropped
                "q2".into(),
                "q3".into(),
                "q4".into(),
                "q5".into(), // beyond the cap
            ],
            risks: vec!["r1".into(), "r2".into(), "r3".into(), "r4".into(), "r5".into()],
            ..Default::default()
        };
        let overlay = validate_polish(&p, many).expect("questions/risks are usable advisory");
        assert_eq!(overlay.questions.len(), MAX_POLISH_QUESTIONS);
        assert_eq!(overlay.risks.len(), MAX_POLISH_RISKS);
        assert!(overlay.questions.iter().all(|q| !q.trim().is_empty()));
    }

    #[test]
    fn validate_polish_returns_none_when_nothing_usable() {
        let p = multi_proposal();
        let empty = PolishSuggestion::default();
        assert!(validate_polish(&p, empty).is_none());
        // A whitespace-only summary with no other content is also nothing usable.
        let blank = PolishSuggestion {
            summary: Some("   ".into()),
            ..Default::default()
        };
        assert!(validate_polish(&p, blank).is_none());
    }

    #[test]
    fn finalize_polish_attaches_model_on_success_and_none_on_error() {
        let cfg = AiConfig::from_parts(Some("k".into()), Some("m/x".into()), false, None);
        let p = multi_proposal();
        // An LLM error -> no overlay; the deterministic proposal stays unpolished.
        let err: Result<PolishSuggestion, String> = Err("timeout".into());
        assert!(finalize_polish(&cfg, &p, err).is_none());
        // A valid suggestion -> overlay carries the model id for provenance.
        let ok = Ok(PolishSuggestion {
            summary: Some("tidy".into()),
            ..Default::default()
        });
        let overlay = finalize_polish(&cfg, &p, ok).expect("valid suggestion yields an overlay");
        assert_eq!(overlay.model.as_deref(), Some("m/x"));
    }

    #[tokio::test]
    async fn polish_proposal_skips_with_no_network_when_brain_is_not_live() {
        // No key configured: the public entry point returns None without any HTTP
        // call, so the deterministic preview is returned unchanged (LLM unavailable
        // -> fallback). Single-step proposals skip the same way.
        let off = AiConfig::from_parts(None, None, false, None);
        assert!(polish_proposal(&off, &multi_proposal()).await.is_none());

        let live = AiConfig::from_parts(Some("k".into()), None, false, None);
        let single = PrimeProposal {
            multi_step: false,
            steps: vec![],
            agents: vec![],
            ..multi_proposal()
        };
        assert!(polish_proposal(&live, &single).await.is_none());
    }

    #[test]
    fn parse_polish_json_lifts_object_from_surrounding_prose() {
        let text = "Sure! Here is the JSON:\n{\"summary\":\"hi\",\"questions\":[\"q\"]}\nHope it helps.";
        let parsed = parse_polish_json(text).expect("a JSON object is present");
        assert_eq!(parsed.summary.as_deref(), Some("hi"));
        assert_eq!(parsed.questions, vec!["q".to_string()]);
        // No object at all -> a stable, secret-free error (caller falls back).
        assert!(parse_polish_json("no json here").is_err());
    }

    // --- CLI-brain proposal polish (advisory, presentation-only) -----------

    #[test]
    fn compose_polish_prompt_carries_steps_and_no_structural_change_rule() {
        let p = multi_proposal();
        let prompt = compose_polish_prompt(&p);
        // Pins the wording-only / no-structural-change contract.
        assert!(prompt.contains("refining the WORDING"));
        assert!(prompt.contains("MUST NOT add, remove, reorder"));
        assert!(prompt.contains("MUST NOT change which agent"));
        // Demands a single JSON object (no prose / code fences).
        assert!(prompt.contains("SINGLE JSON object"));
        assert!(prompt.contains("\"summary\""));
        // Grounds the model in the authoritative steps and agents.
        assert!(prompt.contains("research the options"));
        assert!(prompt.contains("research-agent"));
        assert!(prompt.contains("ship the beta"));
    }

    #[test]
    fn polish_from_cli_text_accepts_valid_json_and_stamps_label() {
        let p = multi_proposal();
        // A clean JSON object (already lifted out of the adapter envelope) with an
        // exact 1:1 step match is accepted; the CLI label rides along as provenance.
        let text = r#"{"summary":"A clear two-stage path.",
            "steps":[{"index":1,"title":"Survey the options"},{"index":2,"title":"Build a prototype"}],
            "questions":["Which platform first?"],"risks":["Scope creep."]}"#;
        let overlay = polish_from_cli_text(&p, text, "Claude CLI")
            .expect("a valid suggestion produces an overlay");
        assert_eq!(overlay.model.as_deref(), Some("Claude CLI"));
        assert_eq!(overlay.step_titles.len(), 2);
        assert_eq!(overlay.step_titles[0].index, 1);
        assert_eq!(overlay.step_titles[0].title, "Survey the options");
        assert_eq!(overlay.summary.as_deref(), Some("A clear two-stage path."));
    }

    #[test]
    fn polish_from_cli_text_tolerates_prose_around_the_json() {
        let p = multi_proposal();
        // A CLI that wraps its JSON in chatter still validates: parse_polish_json
        // lifts the object, then validate_polish enforces the invariants.
        let text = "Sure, here you go:\n{\"summary\":\"tidy plan\"}\nLet me know!";
        let overlay = polish_from_cli_text(&p, text, "Codex CLI").expect("the object is lifted");
        assert_eq!(overlay.summary.as_deref(), Some("tidy plan"));
        assert!(overlay.step_titles.is_empty(), "no steps offered -> none applied");
        assert_eq!(overlay.model.as_deref(), Some("Codex CLI"));
    }

    #[test]
    fn polish_from_cli_text_ignores_malformed_or_objectless_text() {
        let p = multi_proposal();
        // Pure prose with no JSON object at all -> None (deterministic preview stands).
        assert!(polish_from_cli_text(&p, "I think this plan looks great!", "Claude CLI").is_none());
        // A non-object JSON value is not a usable polish.
        assert!(polish_from_cli_text(&p, "[1,2,3]", "Claude CLI").is_none());
        // An empty object carries nothing usable.
        assert!(polish_from_cli_text(&p, "{}", "Claude CLI").is_none());
    }

    #[test]
    fn polish_from_cli_text_rejects_structural_drift_via_the_same_chokepoint() {
        let p = multi_proposal();
        // The CLI tried to ADD a step -> the whole step set is dropped. With no other
        // usable content the overlay is None: the deterministic titles stand.
        let added = r#"{"steps":[{"index":1,"title":"a"},{"index":2,"title":"b"},{"index":3,"title":"c"}]}"#;
        assert!(polish_from_cli_text(&p, added, "Claude CLI").is_none());
        // The CLI tried to DROP a step -> same.
        let dropped = r#"{"steps":[{"index":1,"title":"only one"}]}"#;
        assert!(polish_from_cli_text(&p, dropped, "Claude CLI").is_none());
        // Reordered/renamed indexes (1,3 instead of 1,2) -> titles dropped, but a
        // valid summary still survives (and the titles are empty).
        let reordered = r#"{"summary":"nice","steps":[{"index":1,"title":"a"},{"index":3,"title":"b"}]}"#;
        let overlay = polish_from_cli_text(&p, reordered, "Codex CLI").expect("summary survives");
        assert!(overlay.step_titles.is_empty(), "mismatched indexes drop the titles");
        assert_eq!(overlay.summary.as_deref(), Some("nice"));
        // The authoritative proposal is never mutated by validation.
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[1].agent, "prime");
    }
}
