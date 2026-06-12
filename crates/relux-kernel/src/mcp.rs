//! The loopback MCP discovery client (MCP v1) — a small, blocking JSON-RPC client
//! that runs `tools/list` against an operator-run **loopback HTTP** MCP server.
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 8.2 / section 18 and
//! `docs/HERMES_OPENCLAW_DEEP_AUDIT.md` §9 ("P2 — MCP tool support"). See
//! `docs/mcp.md` for the exact v1 semantics + limitations.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! Read before writing this module:
//!
//! - **Hermes** `reference/hermes-agent-main/tools/mcp_tool.py` — the MCP wire
//!   shape: an `initialize` handshake (`protocolVersion`, `capabilities`,
//!   `clientInfo`) followed by `tools/list` returning `{ "tools": [...] }`, each
//!   tool `{ name, description, inputSchema }`. We speak the same JSON-RPC 2.0
//!   methods. Hermes uses the official `mcp` SDK over stdio/streamable-HTTP/SSE;
//!   Relux v1 deliberately does only the loopback-HTTP, single-POST subset.
//! - **Relix legacy** `crates/relix-runtime/src/nodes/tool/mcp_http.rs` — the prior
//!   streamable-HTTP MCP client (async/reqwest): one POST per JSON-RPC request,
//!   `ensure_initialized` before `list_tools`/`call_tool`, JSON-RPC `error` → honest
//!   failure (never a fake success). We mirror that posture in a blocking
//!   `std::net` client that fits the synchronous kernel tool path (same style as
//!   [`crate::runtime`]).
//!
//! ## Transport (v1 honesty contract)
//!
//! - **Loopback only.** The endpoint is re-validated with
//!   [`relux_core::validate_loopback_url`] on every call (defense in depth) so only
//!   `http://127.0.0.1|localhost|[::1]:<port>` is ever dialed. No https, no remote,
//!   no TLS, no redirects, no stdio subprocess.
//! - **Bounded.** A per-call connect/read/write timeout, a request-body cap, and a
//!   response-body cap (reused from [`crate::runtime`]). Discovered tools are
//!   capped at [`relux_core::MAX_MCP_TOOLS`]; descriptions are sanitized + clamped.
//! - **One POST per JSON-RPC request, session-continuous.** Each JSON-RPC request
//!   is still its own `Connection: close` POST, but a single logical operation
//!   (`initialize` → `tools/list`, or `initialize` → `tools/call`) now carries a
//!   **streamable-HTTP session** across its requests: if the server returns an
//!   `Mcp-Session-Id` response header on `initialize`, that id (bounded + validated
//!   to visible ASCII, never persisted, never surfaced to the UI/API) is echoed on
//!   every subsequent request in the same operation. A server that rejects a stale
//!   session with HTTP 404 triggers **one** bounded clear-and-re-initialize retry;
//!   if it still refuses, the call fails honestly with [`McpClientError`], never a
//!   fabricated result. Sessions are per-operation and in-memory only — there is no
//!   long-lived connection and no cross-call session reuse. See `docs/mcp.md`.
//! - **Honest failures.** A connect failure, timeout, non-2xx status, oversized
//!   body, invalid JSON, or a JSON-RPC `error` becomes a clear [`McpClientError`].

use std::net::TcpStream;
use std::time::Duration;

use relux_core::{parse_loopback_url, McpTool};
use thiserror::Error;

use crate::runtime::{
    self, MAX_REQUEST_BODY_BYTES, RuntimeClientError,
};

/// The MCP protocol version Relux advertises in the `initialize` handshake.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// A failure talking to (or interpreting the response of) a loopback MCP server.
#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("invalid loopback endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("request body too large: {0} bytes (max {MAX_REQUEST_BODY_BYTES})")]
    RequestTooLarge(usize),
    #[error("could not connect to loopback MCP server: {0}")]
    Connect(String),
    #[error("loopback MCP server timed out")]
    Timeout,
    #[error("loopback MCP server I/O error: {0}")]
    Io(String),
    #[error("loopback MCP server returned HTTP {0}")]
    HttpStatus(u16),
    #[error("loopback MCP server response too large")]
    ResponseTooLarge,
    #[error("loopback MCP server returned a malformed HTTP response: {0}")]
    MalformedResponse(String),
    #[error("loopback MCP server returned invalid JSON-RPC: {0}")]
    BadResponse(String),
    #[error("loopback MCP server reported a JSON-RPC error ({code}): {message}")]
    ServerError { code: i64, message: String },
    #[error("loopback MCP tool reported an error: {0}")]
    ToolCallError(String),
    // --- managed-stdio transport (see `crate::mcp_stdio`) -------------------
    #[error("could not spawn managed MCP server: {0}")]
    Spawn(String),
    #[error("managed MCP server exited before responding")]
    ProcessExited,
    #[error("managed MCP server response line exceeds the size cap")]
    StdioLineTooLong,
    #[error("managed MCP server transport error: {0}")]
    Stdio(String),
}

impl From<RuntimeClientError> for McpClientError {
    fn from(e: RuntimeClientError) -> Self {
        match e {
            RuntimeClientError::InvalidUrl(m) => McpClientError::InvalidEndpoint(m),
            RuntimeClientError::RequestTooLarge(n) => McpClientError::RequestTooLarge(n),
            RuntimeClientError::Connect(m) => McpClientError::Connect(m),
            RuntimeClientError::Timeout => McpClientError::Timeout,
            RuntimeClientError::Io(m) => McpClientError::Io(m),
            RuntimeClientError::HttpStatus(s) => McpClientError::HttpStatus(s),
            RuntimeClientError::ResponseTooLarge => McpClientError::ResponseTooLarge,
            RuntimeClientError::MalformedResponse(m) => McpClientError::MalformedResponse(m),
            RuntimeClientError::InvalidJson(m) => McpClientError::BadResponse(m),
            // The loopback-tool envelope errors do not arise on the MCP path, but
            // map them to honest failures rather than panicking.
            RuntimeClientError::ToolError(m) => McpClientError::BadResponse(m),
            RuntimeClientError::MissingOutput => {
                McpClientError::BadResponse("response had no result".to_string())
            }
        }
    }
}

/// Run the MCP `initialize` handshake, then `tools/list`, against the loopback
/// `endpoint`, returning the discovered tools (sanitized + bounded) or an honest
/// [`McpClientError`].
///
/// `endpoint` must already be a validated loopback URL; it is re-validated here.
/// `timeout_ms` bounds connect, read, and write for each request independently. The
/// `initialize` and `tools/list` requests share a streamable-HTTP session (see
/// [`Session`]); a stale-session `404` triggers one bounded re-initialize retry.
pub fn discover_tools(endpoint: &str, timeout_ms: u64) -> Result<Vec<McpTool>, McpClientError> {
    // Re-validate the loopback endpoint on every call (defense in depth).
    let _ =
        parse_loopback_url(endpoint).map_err(|e| McpClientError::InvalidEndpoint(e.to_string()))?;

    run_with_session(endpoint, timeout_ms, |session| {
        // tools/list — the real discovery call, carrying the session established by
        // the handshake. A JSON-RPC `error` is surfaced honestly.
        let result = session.request("tools/list", &serde_json::json!({}))?;
        parse_tools_list(&result)
    })
}

/// Maximum characters of text kept from a `tools/call` result. The whole response
/// body is already capped by [`crate::runtime::read_capped`]; this additionally
/// bounds the model/operator-facing text so a large result never floods the UI.
const MAX_MCP_CALL_TEXT_CHARS: usize = 20_000;

/// Run the MCP `initialize` handshake, then `tools/call`, against the loopback
/// `endpoint`, returning a **shaped, sanitized** result (never the raw JSON-RPC
/// envelope) or an honest [`McpClientError`].
///
/// `endpoint` must already be a validated loopback URL; it is re-validated here on
/// every call (defense in depth). `tool_name` is the MCP tool name and `arguments`
/// is the operator-supplied JSON args object forwarded as the call's `arguments`.
/// `timeout_ms` bounds connect, read, and write for each request independently.
///
/// The returned value mirrors Hermes' `tools/call` shaping
/// (`reference/hermes-agent-main/tools/mcp_tool.py` L2334-2382): the result's text
/// content blocks are concatenated into `result`, and any `structuredContent` is
/// carried alongside. A result flagged `isError` becomes
/// [`McpClientError::ToolCallError`] (the tool ran but reported failure), never a
/// fabricated success. The raw `{ jsonrpc, id, result: { content: [...] } }`
/// envelope is NEVER returned to the caller.
pub fn call_tool(
    endpoint: &str,
    tool_name: &str,
    arguments: &serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, McpClientError> {
    // Re-validate the loopback endpoint on every call (defense in depth).
    let _ =
        parse_loopback_url(endpoint).map_err(|e| McpClientError::InvalidEndpoint(e.to_string()))?;

    run_with_session(endpoint, timeout_ms, |session| {
        // tools/call — forward { name, arguments }, carrying the handshake's session.
        // A non-object `arguments` is sent as-is and the server validates it.
        let params = serde_json::json!({ "name": tool_name, "arguments": arguments });
        let result = session.request("tools/call", &params)?;
        shape_tool_call_result(&result)
    })
}

/// Run the MCP `initialize` handshake, then `resources/list`, against the loopback
/// `endpoint`, returning the discovered resources (sanitized + bounded) or an honest
/// [`McpClientError`].
///
/// `endpoint` must already be a validated loopback URL; it is re-validated here. The
/// requests share a streamable-HTTP session (see [`Session`]); a stale-session `404`
/// triggers one bounded re-initialize retry. Resources are READ-ONLY context — this
/// performs a bounded loopback read and changes nothing on the server.
pub fn list_resources(
    endpoint: &str,
    timeout_ms: u64,
) -> Result<Vec<relux_core::McpResource>, McpClientError> {
    // Re-validate the loopback endpoint on every call (defense in depth).
    let _ =
        parse_loopback_url(endpoint).map_err(|e| McpClientError::InvalidEndpoint(e.to_string()))?;

    run_with_session(endpoint, timeout_ms, |session| {
        let result = session.request("resources/list", &serde_json::json!({}))?;
        parse_resources_list(&result)
    })
}

/// Run the MCP `initialize` handshake, then `resources/read` for `uri`, against the
/// loopback `endpoint`, returning a **shaped, sanitized, secret-redacted**
/// [`relux_core::McpResourceContent`] (never the raw JSON-RPC envelope, never raw
/// binary bytes) or an honest [`McpClientError`].
///
/// `endpoint` must already be a validated loopback URL; it is re-validated here.
/// `uri` is the resource URI forwarded as the `resources/read` `{ uri }` param (the
/// caller is responsible for validating it with
/// [`relux_core::is_valid_mcp_resource_uri`]). Mirrors Hermes' `resources/read`
/// shaping (`reference/hermes-agent-main/tools/mcp_tool.py` L2492-2548): text content
/// blocks are concatenated; a binary (`blob`) block is summarized with an honest
/// marker (its bytes are never decoded or returned). A `resources/read` is a pure
/// read — it performs no action and mutates nothing.
pub fn read_resource(
    endpoint: &str,
    uri: &str,
    timeout_ms: u64,
) -> Result<relux_core::McpResourceContent, McpClientError> {
    // Re-validate the loopback endpoint on every call (defense in depth).
    let _ =
        parse_loopback_url(endpoint).map_err(|e| McpClientError::InvalidEndpoint(e.to_string()))?;

    run_with_session(endpoint, timeout_ms, |session| {
        let params = serde_json::json!({ "uri": uri });
        let result = session.request("resources/read", &params)?;
        shape_resource_read_result(&result, uri)
    })
}

/// Open a streamable-HTTP [`Session`] against `endpoint` (run the `initialize`
/// handshake), then run `op` (the `tools/list` or `tools/call` step) under it. If
/// the server rejects an established session with HTTP `404` (the streamable-HTTP
/// "session not found / expired" signal), clear the session and re-initialize
/// **once**, then retry `op` a single time — bounded, never a retry loop. If the
/// retry still fails, the error is surfaced honestly.
///
/// The session id lives only for the duration of this call: it is captured from the
/// `initialize` response header, echoed on the operation's requests, and dropped
/// when the [`Session`] goes out of scope. It is never persisted and never returned
/// to the caller (and thus never reaches the UI/API).
fn run_with_session<T>(
    endpoint: &str,
    timeout_ms: u64,
    op: impl Fn(&mut Session) -> Result<T, McpClientError>,
) -> Result<T, McpClientError> {
    let mut session = Session::new(endpoint, timeout_ms);
    session.initialize()?;
    match op(&mut session) {
        Ok(value) => Ok(value),
        // Only a 404 *while we hold a session id* means the server invalidated our
        // session; a 404 without one is just an unknown endpoint (honest failure).
        Err(McpClientError::HttpStatus(404)) if session.has_session() => {
            session.reset();
            session.initialize()?;
            op(&mut session)
        }
        Err(other) => Err(other),
    }
}

/// One in-memory streamable-HTTP MCP session: the loopback endpoint, the per-call
/// timeout, a monotonic JSON-RPC id, and the optional `Mcp-Session-Id` handed back
/// by `initialize`. The session id is bounded + validated to visible ASCII before
/// it is ever echoed (so a malformed server value can never inject an HTTP header)
/// and is never logged, persisted, or returned to a caller.
struct Session {
    endpoint: String,
    timeout_ms: u64,
    session_id: Option<String>,
    next_id: u64,
}

impl Session {
    fn new(endpoint: &str, timeout_ms: u64) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            timeout_ms,
            session_id: None,
            next_id: 1,
        }
    }

    /// Whether the server handed us a session id to echo.
    fn has_session(&self) -> bool {
        self.session_id.is_some()
    }

    /// Clear any established session id (used before a bounded re-initialize).
    fn reset(&mut self) {
        self.session_id = None;
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Run the MCP `initialize` handshake, capturing the streamable-HTTP session id
    /// from the response's `Mcp-Session-Id` header (if any), then send the
    /// best-effort `notifications/initialized`. A fresh handshake always drops any
    /// prior session id first (a re-initialize must not reuse the invalidated one).
    fn initialize(&mut self) -> Result<(), McpClientError> {
        self.session_id = None;
        let init_params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "relux", "version": env!("CARGO_PKG_VERSION") },
        });
        let id = self.next_id();
        let (_result, captured) =
            post_jsonrpc(&self.endpoint, id, "initialize", &init_params, self.timeout_ms, None)?;
        // Honor the session only if the server supplied a safe id; a malformed one is
        // ignored (the operation then proceeds session-less, exactly as before).
        self.session_id = captured;
        // notifications/initialized — best effort, carrying the session header so a
        // session-strict server accepts it. A failure here must not abort the op.
        let _ = post_notification(
            &self.endpoint,
            "notifications/initialized",
            self.timeout_ms,
            self.session_id.as_deref(),
        );
        Ok(())
    }

    /// POST one JSON-RPC `method` under this session and return its `result`. The
    /// established session id (if any) is echoed; a response that re-issues one is
    /// honored. A JSON-RPC `error` / transport failure surfaces as [`McpClientError`].
    fn request(
        &mut self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, McpClientError> {
        let id = self.next_id();
        let (result, captured) = post_jsonrpc(
            &self.endpoint,
            id,
            method,
            params,
            self.timeout_ms,
            self.session_id.as_deref(),
        )?;
        if captured.is_some() {
            self.session_id = captured;
        }
        Ok(result)
    }
}

/// Shape a raw `tools/call` JSON-RPC result into a bounded, sanitized value:
/// `{ "result": <text>, "structuredContent"?: <json> }`. An `isError` result is an
/// honest [`McpClientError::ToolCallError`]. Never returns the raw envelope.
///
/// `pub(crate)` so the managed-stdio client ([`crate::mcp_stdio`]) reuses the SAME
/// security-sensitive shaping/redaction — the transport differs, the result handling
/// does not.
pub(crate) fn shape_tool_call_result(
    result: &serde_json::Value,
) -> Result<serde_json::Value, McpClientError> {
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let text = extract_content_text(result.get("content"));
    if is_error {
        let message = if text.is_empty() {
            "MCP tool returned an error".to_string()
        } else {
            text
        };
        return Err(McpClientError::ToolCallError(message));
    }
    let mut out = serde_json::Map::new();
    out.insert("result".to_string(), serde_json::Value::String(text));
    // structuredContent is machine-oriented JSON the tool may supply alongside the
    // text. The whole body is already size-capped, so carry it through verbatim
    // (it is a documented result field, not the JSON-RPC envelope).
    if let Some(structured) = result.get("structuredContent") {
        if !structured.is_null() {
            out.insert("structuredContent".to_string(), structured.clone());
        }
    }
    Ok(serde_json::Value::Object(out))
}

/// Concatenate the text of a `tools/call` result's `content` blocks (the model-
/// oriented payload), sanitized and clamped. A non-text block (image/resource) is
/// summarized as a `[non-text content: <type>]` marker rather than dropped or
/// leaked. Mirrors Hermes' text collection in `mcp_tool.py`.
fn extract_content_text(content: Option<&serde_json::Value>) -> String {
    let Some(items) = content.and_then(|c| c.as_array()) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for block in items {
        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
            parts.push(t.to_string());
            continue;
        }
        let kind = block
            .get("type")
            .and_then(|k| k.as_str())
            .unwrap_or("unknown");
        parts.push(format!("[non-text content: {kind}]"));
    }
    sanitize_result_text(&parts.join("\n"), MAX_MCP_CALL_TEXT_CHARS)
}

/// Light sanitize of MCP result text: drop control characters except newline/tab
/// (so legitimate multi-line text — file contents, logs — is preserved), then
/// clamp to `max` characters. Unlike a description, a result is not collapsed to a
/// single line.
fn sanitize_result_text(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c == '\n' || c == '\t' {
                c
            } else if c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    cleaned.chars().take(max).collect()
}

/// Parse a `tools/list` JSON-RPC result into bounded, sanitized [`McpTool`]s.
///
/// `pub(crate)` so the managed-stdio client ([`crate::mcp_stdio`]) reuses the SAME
/// bounding/sanitizing — only the transport differs.
pub(crate) fn parse_tools_list(result: &serde_json::Value) -> Result<Vec<McpTool>, McpClientError> {
    let tools = result
        .get("tools")
        .and_then(|t| t.as_array())
        .ok_or_else(|| McpClientError::BadResponse("result had no `tools` array".to_string()))?;
    let mut out = Vec::new();
    for tool in tools.iter().take(relux_core::MAX_MCP_TOOLS) {
        let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
            // A tool entry with no name is unusable; skip it rather than fail the
            // whole discovery (one malformed entry shouldn't hide the rest).
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let description = tool
            .get("description")
            .and_then(|d| d.as_str())
            .map(relux_core::sanitize_mcp_tool_description)
            .unwrap_or_default();
        out.push(McpTool {
            name: name.to_string(),
            description,
        });
    }
    Ok(out)
}

/// Parse a `resources/list` JSON-RPC result into bounded, sanitized
/// [`relux_core::McpResource`]s. Mirrors Hermes' `_make_list_resources_handler`
/// (`mcp_tool.py` L2448-2459): collect `{ uri, name, title?, mimeType?, description? }`,
/// skipping any entry without a usable URI. Every string is sanitized + clamped.
fn parse_resources_list(
    result: &serde_json::Value,
) -> Result<Vec<relux_core::McpResource>, McpClientError> {
    let resources = result
        .get("resources")
        .and_then(|t| t.as_array())
        .ok_or_else(|| {
            McpClientError::BadResponse("result had no `resources` array".to_string())
        })?;
    let mut out = Vec::new();
    for res in resources.iter().take(relux_core::MAX_MCP_RESOURCES) {
        // A resource with no usable URI is unaddressable; skip it rather than fail
        // the whole listing (one malformed entry shouldn't hide the rest).
        let Some(uri) = res.get("uri").and_then(|u| u.as_str()) else {
            continue;
        };
        let uri = uri.trim();
        if uri.is_empty() {
            continue;
        }
        let name = res
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| relux_core::sanitize_mcp_text(s, relux_core::MAX_MCP_RESOURCE_NAME_CHARS))
            .unwrap_or_default();
        let title = res
            .get("title")
            .and_then(|t| t.as_str())
            .map(|s| relux_core::sanitize_mcp_text(s, relux_core::MAX_MCP_RESOURCE_NAME_CHARS))
            .filter(|s| !s.is_empty());
        let mime_type = res
            .get("mimeType")
            .and_then(|m| m.as_str())
            .map(|s| relux_core::sanitize_mcp_text(s, relux_core::MAX_MCP_RESOURCE_MIME_CHARS))
            .filter(|s| !s.is_empty());
        let description = res
            .get("description")
            .and_then(|d| d.as_str())
            .map(relux_core::sanitize_mcp_resource_description)
            .unwrap_or_default();
        out.push(relux_core::McpResource {
            uri: uri.chars().take(relux_core::MAX_MCP_RESOURCE_URI_CHARS).collect(),
            name,
            title,
            mime_type,
            description,
        });
    }
    Ok(out)
}

/// Shape a raw `resources/read` JSON-RPC result into a bounded, sanitized,
/// secret-redacted [`relux_core::McpResourceContent`]. Mirrors Hermes'
/// `_make_read_resource_handler` (`mcp_tool.py` L2513-2520): concatenate the text of
/// the `contents` blocks; a binary (`blob`) block is summarized with an honest
/// `[binary content …]` marker (never decoded, never returned). The joined text is
/// sanitized (control chars except newline/tab dropped), **secret-redacted**
/// ([`relux_core::redact_secrets`] — a credential embedded in a resource never leaks
/// verbatim), and clamped. Never returns the raw envelope or raw bytes.
fn shape_resource_read_result(
    result: &serde_json::Value,
    uri: &str,
) -> Result<relux_core::McpResourceContent, McpClientError> {
    let contents = result.get("contents").and_then(|c| c.as_array());
    let mut parts: Vec<String> = Vec::new();
    let mut binary = false;
    let mut mime_type: Option<String> = None;
    if let Some(items) = contents {
        for block in items {
            if mime_type.is_none() {
                if let Some(m) = block.get("mimeType").and_then(|m| m.as_str()) {
                    let m = relux_core::sanitize_mcp_text(m, relux_core::MAX_MCP_RESOURCE_MIME_CHARS);
                    if !m.is_empty() {
                        mime_type = Some(m);
                    }
                }
            }
            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                parts.push(t.to_string());
            } else if block.get("blob").is_some() {
                // Binary content — never decode or surface the bytes; summarize it.
                binary = true;
                let kind = block
                    .get("mimeType")
                    .and_then(|m| m.as_str())
                    .unwrap_or("application/octet-stream");
                parts.push(format!("[binary content omitted: {kind}]"));
            }
        }
    }
    let joined = parts.join("\n");
    let sanitized = sanitize_result_text(&joined, relux_core::MAX_MCP_RESOURCE_TEXT_CHARS);
    // Redact obvious secrets so a credential embedded in a resource body never leaks.
    let text = relux_core::redact_secrets(&sanitized);
    Ok(relux_core::McpResourceContent {
        uri: uri.chars().take(relux_core::MAX_MCP_RESOURCE_URI_CHARS).collect(),
        mime_type,
        text,
        binary,
    })
}

/// POST one JSON-RPC request to the loopback endpoint and return `(result_value,
/// captured_session_id)`, or an honest [`McpClientError`] (transport failure or a
/// JSON-RPC `error`). A single `Connection: close` request, mirroring
/// [`crate::runtime`]. `session_id`, when `Some`, is echoed as the `Mcp-Session-Id`
/// request header; the response's `Mcp-Session-Id` (if any) is returned alongside.
fn post_jsonrpc(
    endpoint: &str,
    id: u64,
    method: &str,
    params: &serde_json::Value,
    timeout_ms: u64,
    session_id: Option<&str>,
) -> Result<(serde_json::Value, Option<String>), McpClientError> {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let (body, captured) = post_raw(endpoint, &envelope, timeout_ms, session_id)?;
    let value = parse_response_body(&body)?;

    if let Some(err) = value.get("error") {
        if !err.is_null() {
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| err.to_string());
            return Err(McpClientError::ServerError { code, message });
        }
    }
    let result = value
        .get("result")
        .cloned()
        .ok_or_else(|| McpClientError::BadResponse("response had no `result` field".to_string()))?;
    Ok((result, captured))
}

/// POST a JSON-RPC notification (no id, no result expected). Best effort: any
/// transport or status failure is swallowed by the caller. `session_id`, when
/// `Some`, is echoed as the `Mcp-Session-Id` request header.
fn post_notification(
    endpoint: &str,
    method: &str,
    timeout_ms: u64,
    session_id: Option<&str>,
) -> Result<(), McpClientError> {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {},
    });
    let _ = post_raw(endpoint, &envelope, timeout_ms, session_id)?;
    Ok(())
}

/// Send one HTTP POST of `envelope` to the loopback endpoint and return
/// `(body_bytes, captured_session_id)`. Re-validates the loopback URL, bounds the
/// request and response, echoes the `Mcp-Session-Id` request header when
/// `session_id` is `Some`, and extracts the response's `Mcp-Session-Id` (validated)
/// for the caller. Uses the shared [`crate::runtime`] HTTP plumbing.
fn post_raw(
    endpoint: &str,
    envelope: &serde_json::Value,
    timeout_ms: u64,
    session_id: Option<&str>,
) -> Result<(Vec<u8>, Option<String>), McpClientError> {
    let parts = parse_loopback_url(endpoint)
        .map_err(|e| McpClientError::InvalidEndpoint(e.to_string()))?;

    let body = serde_json::to_vec(envelope).map_err(|e| McpClientError::Io(e.to_string()))?;
    if body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(McpClientError::RequestTooLarge(body.len()));
    }

    let timeout = Duration::from_millis(timeout_ms);
    let addr = runtime::loopback_socket_addr(&parts.host, parts.port);
    let mut stream =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| McpClientError::Connect(e.to_string()))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| McpClientError::Io(e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| McpClientError::Io(e.to_string()))?;

    let host_header = if parts.host == "::1" {
        format!("[{}]:{}", parts.host, parts.port)
    } else {
        format!("{}:{}", parts.host, parts.port)
    };
    // The MCP endpoint path is the full configured path (no `/invoke` suffix — that
    // is the loopback-TOOL runtime's convention, not MCP's). An empty base path
    // POSTs to "/".
    let path = if parts.path.is_empty() {
        "/".to_string()
    } else {
        parts.path.clone()
    };
    // The streamable-HTTP session header, echoed only when the server gave us a
    // session id. It is already validated to visible ASCII (no CR/LF/space) by
    // `validate_session_id`, so it cannot inject an extra header line.
    let session_header = match session_id {
        Some(sid) => format!("Mcp-Session-Id: {sid}\r\n"),
        None => String::new(),
    };
    // Streamable-HTTP MCP servers may require the dual Accept; we still only parse a
    // single JSON object (or a single SSE `data:` event — see `parse_response_body`).
    let request_head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         {session_header}\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n",
        len = body.len(),
    );

    runtime::write_all(&mut stream, request_head.as_bytes())?;
    runtime::write_all(&mut stream, &body)?;
    use std::io::Write;
    stream.flush().map_err(|e| McpClientError::Io(e.to_string()))?;

    let raw = runtime::read_capped(&mut stream)?;
    // Capture the session id from the response head BEFORE the status check: a
    // streamable-HTTP server may set `Mcp-Session-Id` on the `initialize` 200, but a
    // session 404 carries no useful body — either way the head holds the id.
    let captured = extract_session_id(&raw);
    let (status, response_body) = runtime::parse_http_response(&raw)?;
    if !(200..300).contains(&status) {
        return Err(McpClientError::HttpStatus(status));
    }
    Ok((response_body.to_vec(), captured))
}

/// Max characters kept for a captured `Mcp-Session-Id`. Real session ids (UUIDs,
/// short opaque tokens) are far shorter; a value beyond this is treated as malformed
/// and ignored rather than echoed.
const MAX_MCP_SESSION_ID_CHARS: usize = 512;

/// Extract and validate the `Mcp-Session-Id` response header from a raw HTTP
/// response. Returns `None` when the header is absent, empty, over-long, or carries
/// any byte outside visible ASCII (`0x21..=0x7E`) — the MCP-spec session-id charset.
/// Rejecting a malformed value (rather than echoing it) prevents a hostile server
/// from smuggling CR/LF or control bytes into our next request's headers.
fn extract_session_id(raw: &[u8]) -> Option<String> {
    let head_end = runtime::find_subslice(raw, b"\r\n\r\n")
        .or_else(|| runtime::find_subslice(raw, b"\n\n"))?;
    let head = std::str::from_utf8(&raw[..head_end]).ok()?;
    // Skip the status line and any line without a colon; match the header
    // case-insensitively (HTTP header names are case-insensitive).
    for line in head.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("mcp-session-id") {
                return validate_session_id(value.trim());
            }
        }
    }
    None
}

/// Validate a candidate session id: non-empty, at most [`MAX_MCP_SESSION_ID_CHARS`],
/// and every byte in the visible-ASCII range. Returns the owned id when safe.
fn validate_session_id(s: &str) -> Option<String> {
    if s.is_empty() || s.chars().count() > MAX_MCP_SESSION_ID_CHARS {
        return None;
    }
    if s.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        Some(s.to_string())
    } else {
        None
    }
}

/// Parse a response body into a JSON-RPC envelope value. Accepts either a single
/// JSON object OR a single Server-Sent-Events frame (`data: {json}` lines) — the
/// streamable-HTTP MCP variant. Anything else is an honest [`McpClientError::BadResponse`].
fn parse_response_body(body: &[u8]) -> Result<serde_json::Value, McpClientError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| McpClientError::BadResponse("non-utf8 response body".to_string()))?;
    let trimmed = text.trim();
    // Fast path: a bare JSON object.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Ok(v);
    }
    // SSE path: concatenate the `data:` payload lines and parse that.
    let data: String = trimmed
        .lines()
        .filter_map(|l| l.strip_prefix("data:").map(|d| d.trim()))
        .collect::<Vec<_>>()
        .join("");
    if !data.is_empty() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            return Ok(v);
        }
    }
    Err(McpClientError::BadResponse(format!(
        "could not parse JSON-RPC response: {}",
        trimmed.chars().take(200).collect::<String>()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// Spawn a loopback MCP server that answers each POST in turn from `responses`
    /// (full HTTP responses). Returns the bound endpoint and a receiver of the
    /// request bodies it saw (one per request).
    fn mock_server(responses: Vec<String>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let endpoint = format!("http://127.0.0.1:{}/mcp", addr.port());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let Ok((mut sock, _)) = listener.accept() else {
                    break;
                };
                let body = read_full_request(&mut sock);
                let _ = tx.send(body);
                let _ = sock.write_all(response.as_bytes());
                let _ = sock.flush();
            }
        });
        (endpoint, rx)
    }

    fn read_full_request(sock: &mut TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let header_end = loop {
            if let Some(i) = find_subslice(&data, b"\r\n\r\n") {
                break i + 4;
            }
            match sock.read(&mut buf) {
                Ok(0) => return String::from_utf8_lossy(&data).to_string(),
                Ok(n) => data.extend_from_slice(&buf[..n]),
                Err(_) => return String::from_utf8_lossy(&data).to_string(),
            }
        };
        let headers = String::from_utf8_lossy(&data[..header_end]).to_lowercase();
        let content_length = headers
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while data.len() - header_end < content_length {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => data.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn http_json(value: serde_json::Value) -> String {
        let body = value.to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    /// A 200 JSON response that also sets an `Mcp-Session-Id` response header — the
    /// streamable-HTTP session handshake a session-strict server performs.
    fn http_json_session(value: serde_json::Value, session_id: &str) -> String {
        let body = value.to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: {session_id}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    /// A bare `200 OK {}` (used for the `notifications/initialized` POST).
    fn http_empty_ok() -> String {
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string()
    }

    /// A `404 Not Found` — the streamable-HTTP "session expired / unknown" signal.
    fn http_404() -> String {
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
    }

    fn tools_list_ok() -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "tools": [ { "name": "search", "description": "Search." } ] }
        })
    }

    fn init_ok() -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {} }
        })
    }

    #[test]
    fn discovers_tools_after_initialize() {
        let responses = vec![
            http_json(init_ok()),
            // notifications/initialized — any 200, body ignored
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            http_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "result": { "tools": [
                    { "name": "search", "description": "Search the index." },
                    { "name": "read", "description": "Read a file." }
                ]}
            })),
        ];
        let (endpoint, rx) = mock_server(responses);
        let tools = discover_tools(&endpoint, 2_000).expect("discovery ok");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "search");
        assert_eq!(tools[0].description, "Search the index.");
        // The first request must be the initialize handshake.
        let first = rx.recv().unwrap();
        assert!(first.contains("initialize"), "first req: {first}");
    }

    #[test]
    fn parses_sse_framed_response() {
        let tools = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "result": { "tools": [ { "name": "ping" } ] }
        });
        let sse = format!("event: message\r\ndata: {}\r\n\r\n", tools);
        let sse_resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            sse.len(),
            sse
        );
        let responses = vec![
            http_json(init_ok()),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            sse_resp,
        ];
        let (endpoint, _rx) = mock_server(responses);
        let tools = discover_tools(&endpoint, 2_000).expect("sse discovery ok");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "ping");
    }

    #[test]
    fn jsonrpc_error_is_surfaced_honestly() {
        let responses = vec![
            http_json(init_ok()),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            http_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "error": { "code": -32601, "message": "method not found" }
            })),
        ];
        let (endpoint, _rx) = mock_server(responses);
        let err = discover_tools(&endpoint, 2_000).unwrap_err();
        assert!(matches!(
            err,
            McpClientError::ServerError { code: -32601, .. }
        ));
    }

    #[test]
    fn non_200_on_initialize_is_an_error() {
        let responses =
            vec!["HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()];
        let (endpoint, _rx) = mock_server(responses);
        let err = discover_tools(&endpoint, 2_000).unwrap_err();
        assert!(matches!(err, McpClientError::HttpStatus(500)));
    }

    #[test]
    fn invalid_json_is_an_error_not_a_fake_list() {
        let responses = vec![
            http_json(init_ok()),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json".to_string(),
        ];
        let (endpoint, _rx) = mock_server(responses);
        let err = discover_tools(&endpoint, 2_000).unwrap_err();
        assert!(matches!(err, McpClientError::BadResponse(_)));
    }

    #[test]
    fn non_loopback_endpoint_is_rejected_before_dialing() {
        let err = discover_tools("https://mcp.example.com/mcp", 500).unwrap_err();
        assert!(matches!(err, McpClientError::InvalidEndpoint(_)));
    }

    #[test]
    fn connect_failure_is_honest() {
        // Nothing is listening here; connect must fail clearly (no fake success).
        let err = discover_tools("http://127.0.0.1:1/mcp", 500).unwrap_err();
        assert!(matches!(
            err,
            McpClientError::Connect(_) | McpClientError::Timeout
        ));
    }

    fn call_ok(result: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "result": result })
    }

    #[test]
    fn call_tool_shapes_text_content_and_forwards_args() {
        let responses = vec![
            http_json(init_ok()),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            http_json(call_ok(serde_json::json!({
                "content": [ { "type": "text", "text": "hello world" } ],
                "isError": false
            }))),
        ];
        let (endpoint, rx) = mock_server(responses);
        let out = call_tool(&endpoint, "say", &serde_json::json!({ "name": "alice" }), 2_000)
            .expect("call ok");
        // Shaped result — NEVER the raw { jsonrpc, id, result } envelope.
        assert_eq!(out, serde_json::json!({ "result": "hello world" }));
        assert!(out.get("jsonrpc").is_none());
        // The initialize + tools/call requests were seen; the call carries args.
        let _init = rx.recv().unwrap();
        let _notif = rx.recv().unwrap();
        let call = rx.recv().unwrap();
        assert!(call.contains("tools/call"), "call req: {call}");
        assert!(call.contains("alice"), "call req carried args: {call}");
    }

    #[test]
    fn call_tool_carries_structured_content() {
        let responses = vec![
            http_json(init_ok()),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            http_json(call_ok(serde_json::json!({
                "content": [ { "type": "text", "text": "ok" } ],
                "structuredContent": { "count": 3 }
            }))),
        ];
        let (endpoint, _rx) = mock_server(responses);
        let out = call_tool(&endpoint, "count", &serde_json::json!({}), 2_000).unwrap();
        assert_eq!(out["result"], "ok");
        assert_eq!(out["structuredContent"], serde_json::json!({ "count": 3 }));
    }

    #[test]
    fn call_tool_iserror_is_an_honest_failure() {
        let responses = vec![
            http_json(init_ok()),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            http_json(call_ok(serde_json::json!({
                "content": [ { "type": "text", "text": "boom" } ],
                "isError": true
            }))),
        ];
        let (endpoint, _rx) = mock_server(responses);
        let err = call_tool(&endpoint, "explode", &serde_json::json!({}), 2_000).unwrap_err();
        assert!(matches!(err, McpClientError::ToolCallError(m) if m.contains("boom")));
    }

    #[test]
    fn call_tool_summarizes_non_text_content() {
        let responses = vec![
            http_json(init_ok()),
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string(),
            http_json(call_ok(serde_json::json!({
                "content": [ { "type": "image", "data": "…", "mimeType": "image/png" } ]
            }))),
        ];
        let (endpoint, _rx) = mock_server(responses);
        let out = call_tool(&endpoint, "snap", &serde_json::json!({}), 2_000).unwrap();
        assert_eq!(out["result"], "[non-text content: image]");
    }

    #[test]
    fn call_tool_rejects_non_loopback_before_dialing() {
        let err = call_tool("https://evil.example.com/mcp", "x", &serde_json::json!({}), 500)
            .unwrap_err();
        assert!(matches!(err, McpClientError::InvalidEndpoint(_)));
    }

    #[test]
    fn session_id_is_captured_on_initialize_and_echoed_on_later_requests() {
        // A session-strict server: initialize hands back a session id, and the
        // follow-up tools/list must carry it back as the `Mcp-Session-Id` header.
        let responses = vec![
            http_json_session(init_ok(), "sess-abc123"),
            http_empty_ok(),
            http_json(tools_list_ok()),
        ];
        let (endpoint, rx) = mock_server(responses);
        let tools = discover_tools(&endpoint, 2_000).expect("discovery ok");
        assert_eq!(tools.len(), 1);

        let _init = rx.recv().unwrap();
        let notif = rx.recv().unwrap();
        let list = rx.recv().unwrap();
        // The session is echoed on both the notification and the real call.
        assert!(notif.contains("Mcp-Session-Id: sess-abc123"), "notif: {notif}");
        assert!(list.contains("Mcp-Session-Id: sess-abc123"), "list: {list}");
    }

    #[test]
    fn call_tool_carries_the_session_established_at_initialize() {
        let responses = vec![
            http_json_session(init_ok(), "sess-xyz"),
            http_empty_ok(),
            http_json(call_ok(serde_json::json!({
                "content": [ { "type": "text", "text": "ok" } ]
            }))),
        ];
        let (endpoint, rx) = mock_server(responses);
        let out = call_tool(&endpoint, "say", &serde_json::json!({}), 2_000).expect("call ok");
        assert_eq!(out["result"], "ok");
        let _init = rx.recv().unwrap();
        let _notif = rx.recv().unwrap();
        let call = rx.recv().unwrap();
        assert!(call.contains("tools/call"), "call: {call}");
        assert!(call.contains("Mcp-Session-Id: sess-xyz"), "call carried session: {call}");
    }

    #[test]
    fn invalid_session_triggers_one_bounded_reinitialize_then_succeeds() {
        // First session (S1) is rejected with 404 on tools/list; the client must
        // clear it, re-initialize ONCE (getting S2), and retry tools/list with S2.
        let responses = vec![
            http_json_session(init_ok(), "S1"), // initialize → S1
            http_empty_ok(),                    // notifications/initialized
            http_404(),                         // tools/list with S1 → expired
            http_json_session(init_ok(), "S2"), // re-initialize → S2
            http_empty_ok(),                    // notifications/initialized
            http_json(tools_list_ok()),         // tools/list with S2 → ok
        ];
        let (endpoint, rx) = mock_server(responses);
        let tools = discover_tools(&endpoint, 2_000).expect("recovers after re-init");
        assert_eq!(tools.len(), 1);

        // Six requests were made; the FINAL tools/list carried the fresh S2 (never
        // the invalidated S1).
        let reqs: Vec<String> = (0..6).map(|_| rx.recv().unwrap()).collect();
        let final_list = reqs.last().unwrap();
        assert!(final_list.contains("tools/list"), "final: {final_list}");
        assert!(final_list.contains("Mcp-Session-Id: S2"), "final carried S2: {final_list}");
        assert!(!final_list.contains("S1"), "final must not reuse S1: {final_list}");
    }

    #[test]
    fn persistent_invalid_session_fails_honestly_after_one_retry() {
        // The server rejects every session: the single bounded re-init still 404s,
        // so the call fails honestly rather than retrying forever or faking a list.
        let responses = vec![
            http_json_session(init_ok(), "S1"),
            http_empty_ok(),
            http_404(), // tools/list with S1 → 404
            http_json_session(init_ok(), "S2"),
            http_empty_ok(),
            http_404(), // tools/list with S2 → 404 again
        ];
        let (endpoint, _rx) = mock_server(responses);
        let err = discover_tools(&endpoint, 2_000).unwrap_err();
        assert!(matches!(err, McpClientError::HttpStatus(404)), "got {err:?}");
    }

    #[test]
    fn malformed_session_id_is_ignored_not_echoed() {
        // A server hands back a session id containing a space (outside the visible-
        // ASCII session charset). It must NOT be echoed (header-injection guard); the
        // op proceeds session-less and still works against a lenient server.
        let bad = http_json_session(init_ok(), "bad value");
        let responses = vec![bad, http_empty_ok(), http_json(tools_list_ok())];
        let (endpoint, rx) = mock_server(responses);
        let tools = discover_tools(&endpoint, 2_000).expect("discovery ok");
        assert_eq!(tools.len(), 1);
        let _init = rx.recv().unwrap();
        let _notif = rx.recv().unwrap();
        let list = rx.recv().unwrap();
        // No session header was echoed (the malformed value was dropped).
        assert!(!list.contains("Mcp-Session-Id"), "no session echoed: {list}");
    }

    #[test]
    fn validate_session_id_rejects_unsafe_values() {
        assert_eq!(validate_session_id("abc-123_DEF"), Some("abc-123_DEF".to_string()));
        assert_eq!(validate_session_id(""), None);
        assert_eq!(validate_session_id("has space"), None);
        assert_eq!(validate_session_id("crlf\r\ninjected"), None);
        assert_eq!(validate_session_id(&"a".repeat(MAX_MCP_SESSION_ID_CHARS + 1)), None);
    }

    // --- MCP resources (resources/list + resources/read) -------------------

    #[test]
    fn lists_resources_after_initialize() {
        let responses = vec![
            http_json(init_ok()),
            http_empty_ok(),
            http_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "result": { "resources": [
                    { "uri": "file:///notes.md", "name": "notes",
                      "description": "Daily notes.", "mimeType": "text/markdown" },
                    { "uri": "mem://record/7", "name": "record-7" },
                    { "name": "no-uri-skipped" }
                ]}
            })),
        ];
        let (endpoint, rx) = mock_server(responses);
        let resources = list_resources(&endpoint, 2_000).expect("list ok");
        // The entry with no URI is skipped, not fatal.
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].uri, "file:///notes.md");
        assert_eq!(resources[0].name, "notes");
        assert_eq!(resources[0].mime_type.as_deref(), Some("text/markdown"));
        assert_eq!(resources[0].description, "Daily notes.");
        let first = rx.recv().unwrap();
        assert!(first.contains("initialize"), "first req: {first}");
    }

    #[test]
    fn read_resource_shapes_text_and_redacts_secrets() {
        let responses = vec![
            http_json(init_ok()),
            http_empty_ok(),
            http_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "result": { "contents": [
                    { "uri": "file:///c.txt", "mimeType": "text/plain",
                      "text": "line one\napi_key=sk-supersecretvalue1234567890" }
                ]}
            })),
        ];
        let (endpoint, rx) = mock_server(responses);
        let content = read_resource(&endpoint, "file:///c.txt", 2_000).expect("read ok");
        assert_eq!(content.uri, "file:///c.txt");
        assert_eq!(content.mime_type.as_deref(), Some("text/plain"));
        assert!(!content.binary);
        // The legit prose survives; the embedded secret is redacted (never verbatim).
        assert!(content.text.contains("line one"), "text: {}", content.text);
        assert!(
            !content.text.contains("sk-supersecretvalue1234567890"),
            "secret must be redacted: {}",
            content.text
        );
        let _init = rx.recv().unwrap();
        let _notif = rx.recv().unwrap();
        let read = rx.recv().unwrap();
        assert!(read.contains("resources/read"), "read req: {read}");
        assert!(read.contains("file:///c.txt"), "read req carried uri: {read}");
    }

    #[test]
    fn read_resource_summarizes_binary_content_honestly() {
        let responses = vec![
            http_json(init_ok()),
            http_empty_ok(),
            http_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "result": { "contents": [
                    { "uri": "file:///img.png", "mimeType": "image/png", "blob": "aGVsbG8=" }
                ]}
            })),
        ];
        let (endpoint, _rx) = mock_server(responses);
        let content = read_resource(&endpoint, "file:///img.png", 2_000).expect("read ok");
        assert!(content.binary, "binary block must set the binary flag");
        // The raw base64 bytes are NEVER surfaced; an honest marker is.
        assert!(content.text.contains("[binary content omitted: image/png]"), "text: {}", content.text);
        assert!(!content.text.contains("aGVsbG8="), "raw bytes leaked: {}", content.text);
    }

    #[test]
    fn list_resources_carries_the_session_established_at_initialize() {
        let responses = vec![
            http_json_session(init_ok(), "rsess-1"),
            http_empty_ok(),
            http_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "result": { "resources": [] }
            })),
        ];
        let (endpoint, rx) = mock_server(responses);
        let resources = list_resources(&endpoint, 2_000).expect("list ok");
        assert!(resources.is_empty());
        let _init = rx.recv().unwrap();
        let _notif = rx.recv().unwrap();
        let list = rx.recv().unwrap();
        assert!(list.contains("resources/list"), "list: {list}");
        assert!(list.contains("Mcp-Session-Id: rsess-1"), "list carried session: {list}");
    }

    #[test]
    fn resource_calls_reject_non_loopback_before_dialing() {
        let err = list_resources("https://evil.example.com/mcp", 500).unwrap_err();
        assert!(matches!(err, McpClientError::InvalidEndpoint(_)));
        let err = read_resource("https://evil.example.com/mcp", "file:///x", 500).unwrap_err();
        assert!(matches!(err, McpClientError::InvalidEndpoint(_)));
    }

    #[test]
    fn resources_list_jsonrpc_error_is_surfaced_honestly() {
        let responses = vec![
            http_json(init_ok()),
            http_empty_ok(),
            http_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 2,
                "error": { "code": -32601, "message": "resources not supported" }
            })),
        ];
        let (endpoint, _rx) = mock_server(responses);
        let err = list_resources(&endpoint, 2_000).unwrap_err();
        assert!(matches!(err, McpClientError::ServerError { code: -32601, .. }));
    }
}
