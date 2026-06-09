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

use std::time::Duration;

use relux_core::{PrimeDisposition, PrimeIntent, PrimeTurn};
use serde::Serialize;

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

// --- Configuration ---------------------------------------------------------

/// Resolved AI configuration for one process.
///
/// The API key is held privately and is never part of any serialized surface -
/// see [`AiStatus`], which is the only thing this config exposes to the wire.
#[derive(Debug, Clone)]
pub struct AiConfig {
    /// Present only when a non-empty `RELUX_OPENROUTER_API_KEY` was set. Private
    /// by construction: nothing serializes or logs this.
    api_key: Option<String>,
    /// The model id to request (resolved, never empty).
    pub model: String,
    /// `true` when `RELUX_LLM_DISABLED` forces deterministic mode.
    pub disabled: bool,
    /// Request timeout in milliseconds (already clamped to a sane range).
    pub timeout_ms: u64,
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
        Self::from_parts(api_key, model, disabled, timeout_ms)
    }

    /// Build a config from already-read parts. Pure (no env access), so config
    /// resolution and defaults are unit-testable without touching process env.
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
        }
    }

    /// Whether the LLM path is actually live: a key is present AND not disabled.
    pub fn enabled(&self) -> bool {
        self.api_key.is_some() && !self.disabled
    }

    /// Whether a key is configured at all (independent of the disabled flag).
    pub fn configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// Build the safe, key-free status surface for `GET /v1/relux/ai/status`.
    pub fn status(&self) -> AiStatus {
        let mode = if self.enabled() {
            AiMode::Openrouter
        } else {
            AiMode::Deterministic
        };
        let reason = if self.enabled() {
            format!(
                "OpenRouter configured; conversational replies use {}. Actions stay deterministic and kernel-grounded.",
                self.model
            )
        } else if self.configured() && self.disabled {
            "An OpenRouter key is set but RELUX_LLM_DISABLED forces deterministic Prime."
                .to_string()
        } else {
            "No OpenRouter API key configured; Prime runs fully deterministic.".to_string()
        };
        AiStatus {
            mode,
            configured: self.configured(),
            disabled: self.disabled,
            model: self.model.clone(),
            timeout_ms: self.timeout_ms,
            reason,
        }
    }
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
}

/// The safe, serializable AI status. Deliberately carries NO key material.
#[derive(Debug, Clone, Serialize)]
pub struct AiStatus {
    pub mode: AiMode,
    /// Whether an API key is present (never the key itself).
    pub configured: bool,
    pub disabled: bool,
    pub model: String,
    pub timeout_ms: u64,
    /// A human-readable, secret-free explanation of the current mode.
    pub reason: String,
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
        || matches!(turn.intent, PrimeIntent::ToolDiscovery)
}

/// Decide the path for a turn given the config. Pure: no env, no network.
pub fn plan_turn(cfg: &AiConfig, turn: &PrimeTurn) -> AiPlan {
    if !cfg.enabled() {
        AiPlan::Deterministic
    } else if is_actionful(turn) {
        AiPlan::DeterministicForAction
    } else {
        AiPlan::Augment
    }
}

/// Shape one Prime reply, optionally via OpenRouter.
///
/// `message` is the user's original message; `turn` is the deterministic kernel
/// outcome (already executed). This never mutates kernel state and only ever
/// reads `turn.reply` as grounded facts for the model to rephrase.
pub async fn shape_reply(cfg: &AiConfig, message: &str, turn: &PrimeTurn) -> AiOutcome {
    match plan_turn(cfg, turn) {
        AiPlan::Deterministic => AiOutcome::deterministic(turn.reply.clone()),
        AiPlan::DeterministicForAction => AiOutcome::deterministic_for_action(turn.reply.clone()),
        AiPlan::Augment => {
            let messages = build_messages(message, &turn.reply);
            let result = request_completion(cfg, messages).await;
            outcome_for_augment(cfg, turn.reply.clone(), result)
        }
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

// --- Prompt construction ---------------------------------------------------

/// Build the chat messages. The system prompt pins Prime's identity and the hard
/// rule that the model must not claim it performed any action; the deterministic
/// reply is supplied as grounded facts the model may rely on but must not
/// contradict.
fn build_messages(message: &str, grounded_facts: &str) -> Vec<ChatMessage> {
    const SYSTEM: &str = "You are Prime, the operator of a local Relux control plane. \
Relux is a Codex-like agentic control plane built around tasks, runs, agents, plugins, \
permissions, approvals, and an audit log. Speak naturally and concisely, like a capable \
operator. Hard rules: you did NOT perform any action this turn, so never claim you created \
a task, started a run, installed a plugin, changed a permission, or modified any state. \
If the user wants such a thing, tell them briefly what to say (for example: \
'create a task to summarize the README') instead of pretending it is done. Do not invent \
runs, tasks, plugins, or numbers. Stay consistent with the grounded facts you are given. \
Use plain ASCII.";

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
        max_tokens: MAX_TOKENS,
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
                "configured",
                "disabled",
                "mode",
                "model",
                "reason",
                "timeout_ms"
            ]
        );
        assert!(!obj.contains_key("api_key"));
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
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel");
        // Multi-byte chars must not panic on a non-boundary cut.
        assert_eq!(truncate_chars("aaa", 2).chars().count(), 2);
    }
}
