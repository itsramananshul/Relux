//! The HTTP loopback ToolSet runtime client (Plugin Runtime v1).
//!
//! Spec ref: `docs/RELUX_MASTER_PLAN.md` section 8.2 (ToolSet Plugins) and
//! section 18 (Relux does not auto-run downloaded plugin code). This is the one
//! place the kernel reaches OUT to execute a non-built-in plugin tool, and it does
//! so only against an operator-configured **loopback HTTP server** the operator
//! started themselves. The kernel never shells out to plugin commands, never runs
//! downloaded plugin code in-process, and never calls a remote host.
//!
//! The protocol is deliberately tiny - one stable endpoint:
//!
//! ```text
//! POST <base_url>/invoke
//! Content-Type: application/json
//! { "plugin_id": "...", "tool_name": "...", "input": <json> }
//!
//! 200 OK  { "output": <json> }     -> success
//! 200 OK  { "error": "..." }       -> the tool refused/failed (honest error)
//! ```
//!
//! Safety properties enforced here:
//!
//! - **Loopback only.** The base URL is re-validated with
//!   `relux_core::validate_loopback_url` on every call (defense in depth), so only
//!   `http://127.0.0.1|localhost|[::1]:<port>` is ever dialed.
//! - **Bounded.** A per-call connect/read/write timeout, a request-body cap, and a
//!   response-body cap. JSON in, JSON out. No streaming, no redirects, no TLS.
//! - **Honest failures.** A connection failure, timeout, non-200 status, oversized
//!   body, invalid JSON, or a `{ "error": ... }` payload becomes a clear
//!   [`RuntimeClientError`] - never a fabricated success.
//!
//! The client is a plain blocking call over `std::net::TcpStream` (no async, no
//! extra HTTP/TLS stack): the kernel's tool path is synchronous, and a loopback
//! POST bounded by a timeout is exactly what is needed.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

use relux_core::parse_loopback_url;
use thiserror::Error;

/// Cap the request body we will send (the wrapped `{plugin_id, tool_name, input}`
/// envelope). Tool inputs are small in practice; this refuses a runaway payload.
pub const MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;
/// Cap the total response bytes we will read from the loopback server, so a
/// misbehaving server cannot make the kernel buffer without bound.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// A failure talking to (or interpreting the response of) a loopback runtime.
#[derive(Debug, Error)]
pub enum RuntimeClientError {
    #[error("invalid loopback URL: {0}")]
    InvalidUrl(String),
    #[error("request body too large: {0} bytes (max {MAX_REQUEST_BODY_BYTES})")]
    RequestTooLarge(usize),
    #[error("could not connect to loopback runtime: {0}")]
    Connect(String),
    #[error("loopback runtime timed out")]
    Timeout,
    #[error("loopback runtime I/O error: {0}")]
    Io(String),
    #[error("loopback runtime returned HTTP {0}")]
    HttpStatus(u16),
    #[error("loopback runtime response too large (> {MAX_RESPONSE_BYTES} bytes)")]
    ResponseTooLarge,
    #[error("loopback runtime returned a malformed HTTP response: {0}")]
    MalformedResponse(String),
    #[error("loopback runtime returned invalid JSON: {0}")]
    InvalidJson(String),
    #[error("loopback runtime reported an error: {0}")]
    ToolError(String),
    #[error("loopback runtime response had neither 'output' nor 'error'")]
    MissingOutput,
}

/// Invoke one tool against an operator-run loopback HTTP server and return its
/// `output` JSON, or a [`RuntimeClientError`] explaining the honest failure.
///
/// `base_url` must already be a validated loopback URL; it is re-validated here.
/// `timeout_ms` bounds connect, read, and write independently.
pub fn invoke_http_loopback(
    base_url: &str,
    plugin_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, RuntimeClientError> {
    // Re-validate the loopback URL on every call (defense in depth): even if a
    // bad config slipped past, we never dial a non-loopback or non-http target.
    let parts =
        parse_loopback_url(base_url).map_err(|e| RuntimeClientError::InvalidUrl(e.to_string()))?;

    let envelope = serde_json::json!({
        "plugin_id": plugin_id,
        "tool_name": tool_name,
        "input": input,
    });
    let body = serde_json::to_vec(&envelope).map_err(|e| RuntimeClientError::Io(e.to_string()))?;
    if body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(RuntimeClientError::RequestTooLarge(body.len()));
    }

    let timeout = Duration::from_millis(timeout_ms);
    let addr = loopback_socket_addr(&parts.host, parts.port);
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| RuntimeClientError::Connect(e.to_string()))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| RuntimeClientError::Io(e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| RuntimeClientError::Io(e.to_string()))?;

    // The Host header uses the bracketed form for IPv6.
    let host_header = if parts.host == "::1" {
        format!("[{}]:{}", parts.host, parts.port)
    } else {
        format!("{}:{}", parts.host, parts.port)
    };
    let path = format!("{}/invoke", parts.path); // parts.path is "" or "/prefix"
    let request_head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n",
        len = body.len(),
    );

    write_all(&mut stream, request_head.as_bytes())?;
    write_all(&mut stream, &body)?;
    stream.flush().map_err(map_io)?;

    let raw = read_capped(&mut stream)?;
    let (status, response_body) = parse_http_response(&raw)?;
    if status != 200 {
        return Err(RuntimeClientError::HttpStatus(status));
    }

    let value: serde_json::Value = serde_json::from_slice(response_body)
        .map_err(|e| RuntimeClientError::InvalidJson(e.to_string()))?;

    // An explicit, non-null `error` field is an honest tool failure.
    if let Some(err) = value.get("error") {
        if !err.is_null() {
            let message = err
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| err.to_string());
            return Err(RuntimeClientError::ToolError(message));
        }
    }
    match value.get("output") {
        Some(output) => Ok(output.clone()),
        None => Err(RuntimeClientError::MissingOutput),
    }
}

/// Build the concrete loopback socket address. `localhost` resolves to IPv4
/// loopback; `::1` to IPv6 loopback. The host is already validated as loopback.
///
/// `pub(crate)` so the loopback MCP client ([`crate::mcp`]) reuses the exact same
/// loopback-resolution rule instead of re-deriving it.
pub(crate) fn loopback_socket_addr(host: &str, port: u16) -> SocketAddr {
    let ip: IpAddr = match host {
        "::1" => IpAddr::V6(Ipv6Addr::LOCALHOST),
        // "127.0.0.1" and "localhost" both map to IPv4 loopback.
        _ => IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    SocketAddr::new(ip, port)
}

pub(crate) fn write_all(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), RuntimeClientError> {
    stream.write_all(bytes).map_err(map_io)
}

/// Read the whole response (headers + body) up to [`MAX_RESPONSE_BYTES`]. With
/// `Connection: close` the server closes after writing, so reading to EOF yields
/// the full response; a timeout surfaces as [`RuntimeClientError::Timeout`].
pub(crate) fn read_capped(stream: &mut TcpStream) -> Result<Vec<u8>, RuntimeClientError> {
    let mut out = Vec::with_capacity(1024);
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.len() + n > MAX_RESPONSE_BYTES {
                    return Err(RuntimeClientError::ResponseTooLarge);
                }
                out.extend_from_slice(&buf[..n]);
            }
            Err(e) => return Err(map_io(e)),
        }
    }
    Ok(out)
}

/// Split a raw HTTP/1.1 response into `(status_code, body_slice)`.
///
/// We send `Connection: close`, so the body is simply everything after the blank
/// line that ends the headers - no Content-Length or chunked parsing needed.
pub(crate) fn parse_http_response(raw: &[u8]) -> Result<(u16, &[u8]), RuntimeClientError> {
    let sep = find_subslice(raw, b"\r\n\r\n")
        .map(|i| (i, i + 4))
        .or_else(|| find_subslice(raw, b"\n\n").map(|i| (i, i + 2)));
    let (head_end, body_start) = sep.ok_or_else(|| {
        RuntimeClientError::MalformedResponse("no header/body separator".to_string())
    })?;

    let head = &raw[..head_end];
    let first_line_end = find_subslice(head, b"\r\n")
        .or_else(|| find_subslice(head, b"\n"))
        .unwrap_or(head.len());
    let status_line = std::str::from_utf8(&head[..first_line_end])
        .map_err(|_| RuntimeClientError::MalformedResponse("non-utf8 status line".to_string()))?;

    // "HTTP/1.1 200 OK" -> the second whitespace-separated token is the code.
    let code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| {
            RuntimeClientError::MalformedResponse(format!("bad status line: {status_line:?}"))
        })?;

    Ok((code, &raw[body_start..]))
}

/// Find the first index of `needle` in `haystack`.
pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Map a socket io error to a timeout vs. generic io error.
pub(crate) fn map_io(e: std::io::Error) -> RuntimeClientError {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => RuntimeClientError::Timeout,
        _ => RuntimeClientError::Io(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// Spawn a one-shot loopback server that returns `response` (a full HTTP
    /// response) to the first request, capturing the request body it received.
    /// Returns the bound `base_url` and a receiver that yields the request body.
    fn one_shot_server(response: &'static str) -> (String, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://127.0.0.1:{}", addr.port());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                // Drain the FULL request (headers + Content-Length body) before
                // responding, so the server never closes the socket while the
                // client is still writing the body (which would reset the write).
                let body = read_full_request(&mut sock);
                let _ = tx.send(body);
                let _ = sock.write_all(response.as_bytes());
                let _ = sock.flush();
            }
        });
        (base_url, rx)
    }

    /// Read a complete HTTP request (headers + the Content-Length body) from a
    /// test socket, returning the body bytes. Deterministic: it reads exactly the
    /// advertised body length, so the server never responds (and closes) before
    /// the client has finished writing.
    fn read_full_request(sock: &mut std::net::TcpStream) -> Vec<u8> {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        // 1. Read until the header terminator.
        let header_end = loop {
            if let Some(i) = find_subslice(&data, b"\r\n\r\n") {
                break i + 4;
            }
            match sock.read(&mut buf) {
                Ok(0) => return data,
                Ok(n) => data.extend_from_slice(&buf[..n]),
                Err(_) => return data,
            }
        };
        // 2. Parse Content-Length and read the rest of the body.
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
        data[header_end..].to_vec()
    }

    #[test]
    fn successful_invoke_returns_output_and_sends_envelope() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 32\r\nConnection: close\r\n\r\n{\"output\":{\"echoed\":\"hello\"}}\n\n";
        let (base_url, rx) = one_shot_server(response);
        let out = invoke_http_loopback(
            &base_url,
            "relux-tools-demo",
            "demo.ping",
            &serde_json::json!({ "message": "hello" }),
            2_000,
        )
        .expect("invoke ok");
        assert_eq!(out, serde_json::json!({ "echoed": "hello" }));

        // The server received the wrapped envelope with all three fields.
        let body = rx.recv().unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["plugin_id"], "relux-tools-demo");
        assert_eq!(sent["tool_name"], "demo.ping");
        assert_eq!(sent["input"], serde_json::json!({ "message": "hello" }));
    }

    #[test]
    fn error_payload_is_surfaced_honestly() {
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 26\r\nConnection: close\r\n\r\n{\"error\":\"bad request\"}\n";
        let (base_url, _rx) = one_shot_server(response);
        let err = invoke_http_loopback(
            &base_url,
            "relux-tools-demo",
            "demo.ping",
            &serde_json::json!({}),
            2_000,
        )
        .unwrap_err();
        assert!(matches!(err, RuntimeClientError::ToolError(m) if m == "bad request"));
    }

    #[test]
    fn invalid_json_is_an_error_not_a_fake_success() {
        let response =
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nnot-json";
        let (base_url, _rx) = one_shot_server(response);
        let err = invoke_http_loopback(
            &base_url,
            "relux-tools-demo",
            "demo.ping",
            &serde_json::json!({}),
            2_000,
        )
        .unwrap_err();
        assert!(matches!(err, RuntimeClientError::InvalidJson(_)));
    }

    #[test]
    fn non_200_status_is_an_error() {
        let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (base_url, _rx) = one_shot_server(response);
        let err = invoke_http_loopback(
            &base_url,
            "relux-tools-demo",
            "demo.ping",
            &serde_json::json!({}),
            2_000,
        )
        .unwrap_err();
        assert!(matches!(err, RuntimeClientError::HttpStatus(500)));
    }

    #[test]
    fn missing_output_field_is_an_error() {
        let response =
            "HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{\"other\":123}";
        let (base_url, _rx) = one_shot_server(response);
        let err = invoke_http_loopback(
            &base_url,
            "relux-tools-demo",
            "demo.ping",
            &serde_json::json!({}),
            2_000,
        )
        .unwrap_err();
        assert!(matches!(err, RuntimeClientError::MissingOutput));
    }

    #[test]
    fn invalid_url_is_rejected_before_dialing() {
        let err = invoke_http_loopback(
            "https://example.com:443",
            "p",
            "t",
            &serde_json::json!({}),
            500,
        )
        .unwrap_err();
        assert!(matches!(err, RuntimeClientError::InvalidUrl(_)));
    }

    #[test]
    fn connect_failure_is_honest() {
        // Nothing is listening on this loopback port; connect must fail clearly.
        let err = invoke_http_loopback(
            "http://127.0.0.1:1",
            "p",
            "t",
            &serde_json::json!({}),
            500,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            RuntimeClientError::Connect(_) | RuntimeClientError::Timeout
        ));
    }
}
