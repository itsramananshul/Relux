//! MCP **sampling** (`sampling/createMessage`) — gated, default-deny client handling.
//!
//! MCP sampling **inverts the trust direction** of every other MCP method: instead of
//! Relux driving the server (`tools/call`, `resources/read`, `prompts/get`), the server
//! — mid-operation, over a managed-stdio **session** — sends a `sampling/createMessage`
//! REQUEST *back to Relux* and blocks until Relux runs its OWN configured LLM and returns
//! the completion. That is powerful and dangerous: a hostile/compromised server could
//! drive Relux's model, burn the operator's provider budget, or try to exfiltrate
//! secrets. Relux therefore treats sampling **fail-closed**:
//!
//! 1. **Default deny.** Sampling is disabled per server until the operator explicitly
//!    enables it (`McpServerConfig.sampling.enabled`) AND a Prime/AI provider is
//!    configured. The capability is **never advertised** in the `initialize` handshake
//!    unless [`SamplingContext::serviceable`] — so a spec-compliant server will not even
//!    try when it is off.
//! 2. **Clean refusal, never a hang.** Before this module, a server→client request was
//!    silently drained by the stdio client (it is not the response we are waiting for),
//!    so the server would block until the per-call timeout killed it. Now every inbound
//!    server request is handled here: a disabled / no-provider / malformed sampling
//!    request gets an immediate, honest JSON-RPC **error** response, and any *other*
//!    server-initiated method gets a clean `-32601` — the server degrades instead of
//!    hanging.
//! 3. **The provider key never reaches the server.** The completion is produced by a
//!    [`Sampler`] the kernel builds from the resolved [`crate::ai::AiConfig`] (the key is
//!    held privately on that config, sourced by secret reference). Only the completion
//!    **text** — clamped and [`relux_core::redact_secrets`]-redacted — is returned to the
//!    server. No tool calls, no task/run mutation: this is a single bounded text
//!    completion and nothing else.
//! 4. **Bounded + audited.** Input (system + messages) and output are bounded; every
//!    request records a [`relux_core::McpSamplingAuditRecord`] (decision + counts +
//!    model, NEVER any plaintext) on a process-global tail the operator can read.
//!
//! This module is pure and synchronous: it decides, bounds, redacts, shapes the
//! JSON-RPC response, and audits. The transport (the managed-stdio pump in
//! [`crate::mcp_stdio`]) calls [`handle_inbound_request`] when it reads a server request
//! while waiting for its own response.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use relux_core::{
    redact_secrets, McpSamplingAuditRecord, MAX_MCP_SAMPLING_AUDIT_RECORDS,
    MAX_MCP_SAMPLING_INPUT_CHARS, MAX_MCP_SAMPLING_MAX_TOKENS, MAX_MCP_SAMPLING_MESSAGES,
    MAX_MCP_SAMPLING_MESSAGE_CHARS, MAX_MCP_SAMPLING_OUTPUT_CHARS, MAX_MCP_SAMPLING_SYSTEM_CHARS,
    SAMPLING_DECISION_ALLOWED, SAMPLING_DECISION_BOUNDS_ERROR,
    SAMPLING_DECISION_DENIED_NO_PROVIDER, SAMPLING_DECISION_DENIED_POLICY,
    SAMPLING_DECISION_PROVIDER_ERROR,
};

// JSON-RPC error codes returned to a server for a refused/failed sampling request.
// The application band (-32000..=-32099) is used for the policy/provider outcomes; the
// standard `-32601` (method not found) is used for an unsupported server-initiated
// method, which is the honest "we do not offer that".
/// Sampling refused: disabled by operator policy.
pub const SAMPLING_ERR_DENIED_POLICY: i64 = -32001;
/// Sampling refused: enabled but no Prime/AI provider configured.
pub const SAMPLING_ERR_NO_PROVIDER: i64 = -32002;
/// Sampling refused: the request was malformed / over the input bounds.
pub const SAMPLING_ERR_BOUNDS: i64 = -32003;
/// Sampling attempted but the provider call failed (honest runtime failure).
pub const SAMPLING_ERR_PROVIDER: i64 = -32010;
/// An unsupported server-initiated method (anything that is not `sampling/createMessage`).
pub const SAMPLING_ERR_UNSUPPORTED_METHOD: i64 = -32601;

/// One bounded chat message extracted from a sampling request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingMessage {
    /// The role (`user` / `assistant` / `system`); an unknown role maps to `user`.
    pub role: String,
    /// The message text, already clamped to [`MAX_MCP_SAMPLING_MESSAGE_CHARS`].
    pub text: String,
}

/// A bounded sampling request handed to a [`Sampler`]. Everything here is already
/// clamped: the provider never sees an unbounded prompt from a hostile server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingRequest {
    /// The optional system prompt, clamped to [`MAX_MCP_SAMPLING_SYSTEM_CHARS`].
    pub system: Option<String>,
    /// The chat messages, bounded to [`MAX_MCP_SAMPLING_MESSAGES`] and a total of
    /// [`MAX_MCP_SAMPLING_INPUT_CHARS`].
    pub messages: Vec<SamplingMessage>,
    /// The server-requested max output tokens, clamped to [`MAX_MCP_SAMPLING_MAX_TOKENS`].
    pub max_tokens: Option<u32>,
}

impl SamplingRequest {
    /// Total input characters (system + every message text). Used for audit metadata.
    pub fn input_chars(&self) -> usize {
        self.system
            .as_deref()
            .map(|s| s.chars().count())
            .unwrap_or(0)
            + self
                .messages
                .iter()
                .map(|m| m.text.chars().count())
                .sum::<usize>()
    }
}

/// A provider's completion: the assistant text plus the model id that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingCompletion {
    /// The assistant text (the handler clamps + redacts this before returning it).
    pub text: String,
    /// The model id used (surfaced to the server + audit; never a key).
    pub model: String,
}

/// A synchronous sampling provider: given a bounded request, return the completion or a
/// short, secret-free error reason. The kernel builds the production sampler from the
/// resolved [`crate::ai::AiConfig`] (`crate::ai::build_sampling_sampler`); tests inject a
/// deterministic closure. The provider key NEVER crosses into the MCP server — only the
/// clamped, redacted completion text is returned to the server.
pub type Sampler =
    Arc<dyn Fn(&SamplingRequest) -> Result<SamplingCompletion, String> + Send + Sync>;

/// The per-server sampling context carried by a managed-stdio session.
///
/// [`SamplingContext::default`] is the **disabled** context (no capability advertised,
/// every sampling request cleanly refused) — used by the spawn-per-operation fallback
/// (which is not a "session") and by a server with sampling off.
#[derive(Clone, Default)]
pub struct SamplingContext {
    /// Whether the operator enabled sampling for this server.
    pub enabled: bool,
    /// The server id (for audit records).
    pub server_id: String,
    /// The provider, or `None` when no Prime/AI provider is configured (a request then
    /// refuses cleanly with "no provider").
    pub sampler: Option<Sampler>,
}

impl SamplingContext {
    /// The disabled context (the explicit default).
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Whether sampling is actually serviceable: the operator enabled it AND a provider
    /// is wired. The `sampling` capability is advertised in `initialize` only when this
    /// is true, so a compliant server will not send a sampling request we would refuse.
    pub fn serviceable(&self) -> bool {
        self.enabled && self.sampler.is_some()
    }
}

/// Handle one server-initiated JSON-RPC request read off a managed-stdio session while
/// the client waits for its own response. Returns the full JSON-RPC response envelope to
/// write back to the server. Dispatches `sampling/createMessage` to the gated handler;
/// any other server-initiated method gets a clean `-32601` (we offer no other
/// server→client method), so the server degrades rather than hanging.
pub fn handle_inbound_request(
    ctx: &SamplingContext,
    method: &str,
    id: &Value,
    params: Option<&Value>,
) -> Value {
    match method {
        "sampling/createMessage" => handle_create_message(ctx, id, params),
        other => error_envelope(
            id,
            SAMPLING_ERR_UNSUPPORTED_METHOD,
            &format!("server-initiated method not supported: {other}"),
        ),
    }
}

/// The gated `sampling/createMessage` handler: decide (default-deny), bound, call the
/// provider, clamp + redact the output, audit, and shape the JSON-RPC response.
fn handle_create_message(ctx: &SamplingContext, id: &Value, params: Option<&Value>) -> Value {
    if !ctx.enabled {
        record_audit(audit(
            ctx,
            SAMPLING_DECISION_DENIED_POLICY,
            "sampling disabled by operator policy",
            0,
            0,
            None,
        ));
        return error_envelope(
            id,
            SAMPLING_ERR_DENIED_POLICY,
            "MCP sampling is disabled by operator policy for this server",
        );
    }
    let Some(sampler) = ctx.sampler.as_ref() else {
        record_audit(audit(
            ctx,
            SAMPLING_DECISION_DENIED_NO_PROVIDER,
            "no Prime/AI provider configured",
            0,
            0,
            None,
        ));
        return error_envelope(
            id,
            SAMPLING_ERR_NO_PROVIDER,
            "MCP sampling is enabled but no Prime/AI provider is configured",
        );
    };

    let request = match parse_request(params) {
        Ok(r) => r,
        Err(reason) => {
            record_audit(audit(
                ctx,
                SAMPLING_DECISION_BOUNDS_ERROR,
                &reason,
                0,
                0,
                None,
            ));
            return error_envelope(
                id,
                SAMPLING_ERR_BOUNDS,
                &format!("invalid sampling request: {reason}"),
            );
        }
    };
    let input_chars = request.input_chars();

    match sampler(&request) {
        Ok(completion) => {
            // Clamp + redact the completion BEFORE it leaves for the server, so a
            // credential the model echoed never reaches the (possibly hostile) server.
            let text = bound_and_redact_output(&completion.text);
            let output_chars = text.chars().count();
            let model = sanitize_short(&completion.model, 200);
            record_audit(audit(
                ctx,
                SAMPLING_DECISION_ALLOWED,
                "completion returned",
                input_chars,
                output_chars,
                Some(model.clone()),
            ));
            result_envelope(
                id,
                json!({
                    "role": "assistant",
                    "content": { "type": "text", "text": text },
                    "model": model,
                    "stopReason": "endTurn",
                }),
            )
        }
        Err(reason) => {
            // Provider reasons are already short + secret-free by construction, but
            // redact defensively in case a closure leaked something.
            let redacted = sanitize_short(&redact_secrets(&reason), 300);
            record_audit(audit(
                ctx,
                SAMPLING_DECISION_PROVIDER_ERROR,
                &redacted,
                input_chars,
                0,
                None,
            ));
            error_envelope(
                id,
                SAMPLING_ERR_PROVIDER,
                &format!("sampling provider failed: {redacted}"),
            )
        }
    }
}

/// Parse + bound a `sampling/createMessage` params object into a [`SamplingRequest`].
/// Fail-closed: a missing/empty message list is an error (there is nothing to sample).
fn parse_request(params: Option<&Value>) -> Result<SamplingRequest, String> {
    let params = params.ok_or_else(|| "missing params".to_string())?;

    let system = params
        .get("systemPrompt")
        .and_then(|v| v.as_str())
        .map(|s| clamp_chars(s.trim(), MAX_MCP_SAMPLING_SYSTEM_CHARS))
        .filter(|s| !s.is_empty());

    let max_tokens = params
        .get("maxTokens")
        .and_then(|v| v.as_u64())
        .map(|t| (t.min(MAX_MCP_SAMPLING_MAX_TOKENS as u64)) as u32);

    let raw = params
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "messages must be an array".to_string())?;

    // Budget the TOTAL input (system already counted) so a hostile server cannot make
    // Relux run an unbounded prompt against its own provider: include each message in
    // order, truncating the one that crosses the cap and dropping the rest.
    let mut budget = MAX_MCP_SAMPLING_INPUT_CHARS
        .saturating_sub(system.as_deref().map(|s| s.chars().count()).unwrap_or(0));
    let mut messages: Vec<SamplingMessage> = Vec::new();
    for item in raw.iter().take(MAX_MCP_SAMPLING_MESSAGES) {
        if budget == 0 {
            break;
        }
        let role = normalize_role(item.get("role").and_then(|r| r.as_str()).unwrap_or("user"));
        let mut text = clamp_chars(
            &message_content_text(item.get("content")),
            MAX_MCP_SAMPLING_MESSAGE_CHARS,
        );
        if text.is_empty() {
            continue;
        }
        let len = text.chars().count();
        if len > budget {
            text = clamp_chars(&text, budget);
        }
        budget = budget.saturating_sub(text.chars().count());
        messages.push(SamplingMessage { role, text });
    }

    if messages.is_empty() {
        return Err("no usable messages".to_string());
    }
    Ok(SamplingRequest {
        system,
        messages,
        max_tokens,
    })
}

/// Extract the text from a sampling message `content` value: a `{ type:"text", text }`
/// (or `{ text }`) block, a bare string, or an array of blocks joined (a non-text block
/// summarized). Mirrors the prompt-message shaping so the behavior is consistent.
fn message_content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Object(_)) => block_text(content.unwrap()),
        Some(Value::Array(items)) => items
            .iter()
            .map(block_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Text of one content block: its `.text` when present, else a `[non-text content:
/// <type>]` marker.
fn block_text(block: &Value) -> String {
    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
        return text.trim().to_string();
    }
    if let Some(s) = block.as_str() {
        return s.trim().to_string();
    }
    let kind = block
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");
    format!("[non-text content: {kind}]")
}

/// Normalize a role to one of `user` / `assistant` / `system`; anything else → `user`.
fn normalize_role(role: &str) -> String {
    match role.trim().to_ascii_lowercase().as_str() {
        "assistant" => "assistant",
        "system" => "system",
        _ => "user",
    }
    .to_string()
}

/// Redact then clamp the completion text returned to the server.
fn bound_and_redact_output(text: &str) -> String {
    clamp_chars(&redact_secrets(text), MAX_MCP_SAMPLING_OUTPUT_CHARS)
}

/// Clamp a string to at most `max` characters (char-safe).
fn clamp_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Trim + clamp a short label, collapsing control characters.
fn sanitize_short(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    clamp_chars(cleaned.trim(), max)
}

/// A JSON-RPC error response envelope.
fn error_envelope(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

/// A JSON-RPC result response envelope.
fn result_envelope(id: &Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

/// Build an audit record (no plaintext — counts + decision + reason + model only).
fn audit(
    ctx: &SamplingContext,
    decision: &str,
    reason: &str,
    input_chars: usize,
    output_chars: usize,
    model: Option<String>,
) -> McpSamplingAuditRecord {
    McpSamplingAuditRecord {
        server_id: ctx.server_id.clone(),
        decision: decision.to_string(),
        reason: sanitize_short(reason, 300),
        input_chars,
        output_chars,
        model,
    }
}

// --- Process-global audit tail --------------------------------------------------

/// The process-global sampling audit ring. Lives OUTSIDE [`crate::state::KernelState`]
/// (like the managed pool) so the off-lock pump can record a decision without taking the
/// kernel lock. Bounded; carries no plaintext.
fn audit_ring() -> &'static Mutex<VecDeque<McpSamplingAuditRecord>> {
    static RING: OnceLock<Mutex<VecDeque<McpSamplingAuditRecord>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Append one record to the process-global audit tail (bounded to
/// [`MAX_MCP_SAMPLING_AUDIT_RECORDS`], oldest dropped first).
pub fn record_audit(record: McpSamplingAuditRecord) {
    if let Ok(mut ring) = audit_ring().lock() {
        ring.push_back(record);
        while ring.len() > MAX_MCP_SAMPLING_AUDIT_RECORDS {
            ring.pop_front();
        }
    }
}

/// A snapshot of the sampling audit tail (oldest first). Carries no plaintext.
pub fn audit_tail() -> Vec<McpSamplingAuditRecord> {
    audit_ring()
        .lock()
        .map(|ring| ring.iter().cloned().collect())
        .unwrap_or_default()
}

/// Clear the audit tail. Exposed for tests + an operator "clear" action; clearing the
/// diagnostic ring is harmless (it holds no plaintext and drives no decision).
pub fn clear_audit() {
    if let Ok(mut ring) = audit_ring().lock() {
        ring.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_disabled(id: &str) -> SamplingContext {
        SamplingContext {
            enabled: false,
            server_id: id.to_string(),
            sampler: None,
        }
    }

    fn ctx_with(id: &str, sampler: Sampler) -> SamplingContext {
        SamplingContext {
            enabled: true,
            server_id: id.to_string(),
            sampler: Some(sampler),
        }
    }

    fn req(text: &str) -> Value {
        json!({ "messages": [ { "role": "user", "content": { "type": "text", "text": text } } ] })
    }

    fn last_for(server: &str) -> McpSamplingAuditRecord {
        audit_tail()
            .into_iter()
            .rev()
            .find(|r| r.server_id == server)
            .expect("an audit record for the server")
    }

    #[test]
    fn disabled_is_denied_by_policy() {
        let id = json!(1);
        let resp = handle_inbound_request(
            &ctx_disabled("s_policy"),
            "sampling/createMessage",
            &id,
            Some(&req("hi")),
        );
        assert_eq!(resp["error"]["code"], SAMPLING_ERR_DENIED_POLICY);
        assert!(resp.get("result").is_none());
        assert_eq!(
            last_for("s_policy").decision,
            SAMPLING_DECISION_DENIED_POLICY
        );
    }

    #[test]
    fn enabled_without_provider_is_no_provider() {
        let ctx = SamplingContext {
            enabled: true,
            server_id: "s_noprov".to_string(),
            sampler: None,
        };
        let id = json!(2);
        let resp = handle_inbound_request(&ctx, "sampling/createMessage", &id, Some(&req("hi")));
        assert_eq!(resp["error"]["code"], SAMPLING_ERR_NO_PROVIDER);
        assert_eq!(
            last_for("s_noprov").decision,
            SAMPLING_DECISION_DENIED_NO_PROVIDER
        );
    }

    #[test]
    fn allowed_returns_clamped_redacted_completion() {
        // A provider that leaks a fake secret AND overflows the output cap.
        let leak = format!(
            "ok api_key=sk-leakedsecret1234567890 {}",
            "x".repeat(MAX_MCP_SAMPLING_OUTPUT_CHARS + 500)
        );
        let sampler: Sampler = Arc::new(move |_r| {
            Ok(SamplingCompletion {
                text: leak.clone(),
                model: "test/model".to_string(),
            })
        });
        let id = json!(3);
        let resp = handle_inbound_request(
            &ctx_with("s_ok", sampler),
            "sampling/createMessage",
            &id,
            Some(&req("please summarize")),
        );
        let text = resp["result"]["content"]["text"].as_str().unwrap();
        assert!(
            !text.contains("sk-leakedsecret"),
            "secret must be redacted out"
        );
        assert!(
            text.chars().count() <= MAX_MCP_SAMPLING_OUTPUT_CHARS,
            "output clamped"
        );
        assert_eq!(resp["result"]["role"], "assistant");
        assert_eq!(resp["result"]["model"], "test/model");
        let rec = last_for("s_ok");
        assert_eq!(rec.decision, SAMPLING_DECISION_ALLOWED);
        assert!(rec.output_chars > 0 && rec.input_chars > 0);
        assert_eq!(rec.model.as_deref(), Some("test/model"));
    }

    #[test]
    fn empty_messages_is_a_bounds_error() {
        let sampler: Sampler = Arc::new(|_r| {
            Ok(SamplingCompletion {
                text: "unused".to_string(),
                model: "m".to_string(),
            })
        });
        let id = json!(4);
        let params = json!({ "messages": [] });
        let resp = handle_inbound_request(
            &ctx_with("s_bounds", sampler),
            "sampling/createMessage",
            &id,
            Some(&params),
        );
        assert_eq!(resp["error"]["code"], SAMPLING_ERR_BOUNDS);
        assert_eq!(
            last_for("s_bounds").decision,
            SAMPLING_DECISION_BOUNDS_ERROR
        );
    }

    #[test]
    fn input_is_bounded_before_the_provider_sees_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        let sampler: Sampler = Arc::new(|r: &SamplingRequest| {
            SEEN.store(r.input_chars(), Ordering::SeqCst);
            Ok(SamplingCompletion {
                text: "ok".to_string(),
                model: "m".to_string(),
            })
        });
        // Two messages each far larger than the total input cap.
        let big = "y".repeat(MAX_MCP_SAMPLING_INPUT_CHARS * 2);
        let params = json!({ "messages": [
            { "role": "user", "content": { "type": "text", "text": big } },
            { "role": "user", "content": { "type": "text", "text": "tail" } },
        ]});
        let id = json!(5);
        let _ = handle_inbound_request(
            &ctx_with("s_in", sampler),
            "sampling/createMessage",
            &id,
            Some(&params),
        );
        assert!(SEEN.load(Ordering::SeqCst) <= MAX_MCP_SAMPLING_INPUT_CHARS);
    }

    #[test]
    fn unsupported_server_method_is_clean_method_not_found() {
        let id = json!(6);
        let resp = handle_inbound_request(&ctx_disabled("s_x"), "roots/list", &id, None);
        assert_eq!(resp["error"]["code"], SAMPLING_ERR_UNSUPPORTED_METHOD);
    }

    #[test]
    fn provider_failure_is_an_honest_error() {
        let sampler: Sampler = Arc::new(|_r| Err("timeout".to_string()));
        let id = json!(7);
        let resp = handle_inbound_request(
            &ctx_with("s_fail", sampler),
            "sampling/createMessage",
            &id,
            Some(&req("go")),
        );
        assert_eq!(resp["error"]["code"], SAMPLING_ERR_PROVIDER);
        assert_eq!(
            last_for("s_fail").decision,
            SAMPLING_DECISION_PROVIDER_ERROR
        );
    }
}
