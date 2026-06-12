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
//! - **Single-POST subset.** Each JSON-RPC request is its own `Connection: close`
//!   POST. The `initialize` handshake runs first, then `tools/list`. A server that
//!   requires session continuity ACROSS requests (e.g. a streamable-HTTP session
//!   id) is not supported yet — its `tools/list` / `tools/call` fails honestly with
//!   [`McpClientError::ServerError`] / [`McpClientError::BadResponse`], never a
//!   fabricated result. See `docs/mcp.md` for the next slice.
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
/// `timeout_ms` bounds connect, read, and write for each request independently.
pub fn discover_tools(endpoint: &str, timeout_ms: u64) -> Result<Vec<McpTool>, McpClientError> {
    // Re-validate the loopback endpoint on every call (defense in depth).
    let _ = parse_loopback_url(endpoint).map_err(|e| McpClientError::InvalidEndpoint(e.to_string()))?;

    // 1. initialize — required by the MCP spec before any other method. We do not
    //    inspect the server's advertised capabilities in v1; a JSON-RPC `error`
    //    here is surfaced honestly (the server refused to initialize).
    let init_params = serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "relux", "version": env!("CARGO_PKG_VERSION") },
    });
    let _ = post_jsonrpc(endpoint, 1, "initialize", &init_params, timeout_ms)?;

    // 2. notifications/initialized — best effort. Some servers expect it; a failure
    //    here must not abort discovery (it carries no result).
    let _ = post_notification(endpoint, "notifications/initialized", timeout_ms);

    // 3. tools/list — the real discovery call.
    let result = post_jsonrpc(endpoint, 2, "tools/list", &serde_json::json!({}), timeout_ms)?;
    parse_tools_list(&result)
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

    // 1. initialize — required before any other method.
    let init_params = serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "relux", "version": env!("CARGO_PKG_VERSION") },
    });
    let _ = post_jsonrpc(endpoint, 1, "initialize", &init_params, timeout_ms)?;
    // 2. notifications/initialized — best effort (carries no result).
    let _ = post_notification(endpoint, "notifications/initialized", timeout_ms);

    // 3. tools/call — forward { name, arguments }. An object that is not a JSON
    //    object is wrapped under no key; the MCP spec expects an object, so a
    //    non-object is sent as-is and the server validates it.
    let params = serde_json::json!({ "name": tool_name, "arguments": arguments });
    let result = post_jsonrpc(endpoint, 2, "tools/call", &params, timeout_ms)?;
    shape_tool_call_result(&result)
}

/// Shape a raw `tools/call` JSON-RPC result into a bounded, sanitized value:
/// `{ "result": <text>, "structuredContent"?: <json> }`. An `isError` result is an
/// honest [`McpClientError::ToolCallError`]. Never returns the raw envelope.
fn shape_tool_call_result(
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
fn parse_tools_list(result: &serde_json::Value) -> Result<Vec<McpTool>, McpClientError> {
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

/// POST one JSON-RPC request to the loopback endpoint and return its `result`
/// value, or an honest [`McpClientError`] (transport failure or a JSON-RPC
/// `error`). A single `Connection: close` request, mirroring [`crate::runtime`].
fn post_jsonrpc(
    endpoint: &str,
    id: u64,
    method: &str,
    params: &serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, McpClientError> {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let body = post_raw(endpoint, &envelope, timeout_ms)?;
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
    value
        .get("result")
        .cloned()
        .ok_or_else(|| McpClientError::BadResponse("response had no `result` field".to_string()))
}

/// POST a JSON-RPC notification (no id, no result expected). Best effort: any
/// transport or status failure is swallowed by the caller.
fn post_notification(endpoint: &str, method: &str, timeout_ms: u64) -> Result<(), McpClientError> {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {},
    });
    let _ = post_raw(endpoint, &envelope, timeout_ms)?;
    Ok(())
}

/// Send one HTTP POST of `envelope` to the loopback endpoint and return the raw
/// 2xx response body bytes. Re-validates the loopback URL, bounds the request and
/// response, and uses the shared [`crate::runtime`] HTTP plumbing.
fn post_raw(
    endpoint: &str,
    envelope: &serde_json::Value,
    timeout_ms: u64,
) -> Result<Vec<u8>, McpClientError> {
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
    // Streamable-HTTP MCP servers may require the dual Accept; we still only parse a
    // single JSON object (or a single SSE `data:` event — see `parse_response_body`).
    let request_head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
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
    let (status, response_body) = runtime::parse_http_response(&raw)?;
    if !(200..300).contains(&status) {
        return Err(McpClientError::HttpStatus(status));
    }
    Ok(response_body.to_vec())
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
}
