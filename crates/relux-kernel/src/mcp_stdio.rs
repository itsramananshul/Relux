//! The **managed-stdio MCP client** — a small, blocking JSON-RPC client that spawns
//! an operator-confirmed local command (argv only, never a shell) and speaks the MCP
//! `initialize` → `tools/list` / `tools/call` subset over the child's stdin/stdout.
//!
//! Companion to the loopback-HTTP client in [`crate::mcp`]. The transport differs
//! (a spawned subprocess instead of a loopback POST); the **result handling does
//! not** — discovery and tool-call shaping reuse the SAME bounded, secret-redacted
//! shapers ([`crate::mcp::parse_tools_list`], [`crate::mcp::shape_tool_call_result`])
//! so an MCP tool over stdio is governed identically to one over HTTP.
//!
//! ## Reference-driven design (`docs/reference-driven-development.md`, BINDING)
//!
//! Read before writing this module:
//!
//! - **Hermes** `reference/hermes-agent-main/hermes_cli/mcp_config.py` — a server is
//!   `{"url"}` (HTTP) or `{"command","args","env"}` (stdio), spawned via the MCP SDK
//!   over stdio; `_probe_single_server` connects, lists tools, and disconnects
//!   (L167-205). We mirror the **spawn → initialize → list/call → reap** lifecycle,
//!   but go stricter: argv only (no shell), no env stored, no `cwd` override, a hard
//!   line-size + timeout bound, and bounded redacted stderr.
//! - **Relix legacy** `crates/relix-runtime/src/nodes/tool/mcp_stdio.rs` — the prior
//!   async (tokio) stdio MCP client: spawn with `kill_on_drop`, send `initialize` +
//!   `notifications/initialized`, drain server→client notifications until a response
//!   with a matching id, map every spawn/EOF/parse failure to an honest error (never
//!   a fake success). We port that posture to a **blocking** `std::process` client
//!   that fits the synchronous kernel tool path (the same shape as [`crate::mcp`]).
//!
//! ## Process model (spawn-per-operation, bounded, reaped)
//!
//! Each discovery / tool-call **spawns its own child, runs one logical operation
//! (`initialize` → the request), and reaps it** — exactly like the HTTP client opens
//! a fresh `Connection: close` POST per operation. There is no long-lived daemon:
//! the child is killed (via [`StdioChild::drop`]) when the operation ends, so a
//! managed server can never linger, leak, or run between operations. The lifecycle is
//! operator-controlled (a discovery / a gated invocation drives one bounded run) and
//! safe by construction:
//!
//! - **No shell.** The command + args are passed as `argv` to [`std::process::Command`]
//!   (re-validated by [`relux_core::validate_stdio_command`] on every spawn). No
//!   string is ever handed to a shell, so there is no metacharacter-injection surface.
//! - **No env injection / no `cwd` override.** The child inherits the parent
//!   environment unchanged and runs in the parent's working directory; Relux passes
//!   no extra env (storing env would store secrets) and no bypass/danger flag.
//! - **Bounded.** A per-call timeout bounds every request (the child is killed on
//!   expiry); each stdout line is size-capped ([`MAX_STDIO_LINE_BYTES`]); stderr is
//!   drained into a bounded, secret-redacted tail surfaced only on failure.
//! - **Honest failures.** A spawn failure, EOF before a response, an oversized line,
//!   a malformed body, or a JSON-RPC `error` becomes a clear [`McpClientError`] — a
//!   `tools/call` `isError` is a runtime failure, never a fabricated success.

use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use relux_core::McpTool;

use crate::mcp::{parse_tools_list, shape_tool_call_result, McpClientError};

/// The MCP protocol version Relux advertises in the `initialize` handshake (matches
/// the loopback-HTTP client).
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Maximum bytes kept for one MCP response line on stdout. A hostile/runaway server
/// that never emits a newline is bounded here instead of growing the buffer forever.
const MAX_STDIO_LINE_BYTES: usize = 4 * 1024 * 1024;

/// Most stderr lines kept in the bounded failure tail.
const MAX_STDERR_TAIL_LINES: usize = 20;
/// Max characters kept per stderr line in the tail.
const MAX_STDERR_LINE_CHARS: usize = 500;
/// Max characters of the rendered, redacted stderr tail folded into an error.
const MAX_STDERR_TAIL_CHARS: usize = 4_000;

/// Spawn the managed command, run `initialize` → `tools/list`, reap the child, and
/// return the discovered tools (sanitized + bounded) or an honest [`McpClientError`].
///
/// `command` + `args` are re-validated with [`relux_core::validate_stdio_command`] on
/// every call (defense in depth — the registry validated them at registration time).
/// `timeout_ms` bounds each request independently.
pub fn discover_tools(
    command: &str,
    args: &[String],
    timeout_ms: u64,
) -> Result<Vec<McpTool>, McpClientError> {
    run_op(command, args, timeout_ms, |child, timeout| {
        let result = child.request("tools/list", &serde_json::json!({}), timeout)?;
        parse_tools_list(&result)
    })
}

/// Spawn the managed command, run `initialize` → `tools/call`, reap the child, and
/// return a **shaped, sanitized** result (never the raw JSON-RPC envelope) or an
/// honest [`McpClientError`]. A `tools/call` flagged `isError` becomes
/// [`McpClientError::ToolCallError`] — never a fabricated success. The tool name is
/// the caller's responsibility to validate ([`relux_core::is_valid_mcp_tool_name`]).
pub fn call_tool(
    command: &str,
    args: &[String],
    tool_name: &str,
    arguments: &serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, McpClientError> {
    run_op(command, args, timeout_ms, |child, timeout| {
        let params = serde_json::json!({ "name": tool_name, "arguments": arguments });
        let result = child.request("tools/call", &params, timeout)?;
        shape_tool_call_result(&result)
    })
}

/// Spawn the managed command, initialize it, run `op` (one `tools/list` or
/// `tools/call`), then reap the child. On any failure the error is **enriched** with
/// the child's bounded, secret-redacted stderr tail so the operator can see why.
fn run_op<T>(
    command: &str,
    args: &[String],
    timeout_ms: u64,
    op: impl FnOnce(&mut StdioChild, Duration) -> Result<T, McpClientError>,
) -> Result<T, McpClientError> {
    // Re-validate the command on every spawn (defense in depth) so a never-shelled,
    // bounded argv is the only thing we ever hand to the OS.
    relux_core::validate_stdio_command(command, args)
        .map_err(|e| McpClientError::Spawn(e.to_string()))?;
    let timeout = Duration::from_millis(timeout_ms);
    let mut child = StdioChild::spawn(command, args)?;
    let outcome = child
        .initialize(timeout)
        .and_then(|()| op(&mut child, timeout));
    match outcome {
        Ok(value) => Ok(value),
        Err(err) => Err(child.enrich_error(err)),
    }
}

/// One line read off the child's stdout by the reader thread.
enum StdoutLine {
    /// A complete (newline-terminated or final) line of bytes, decoded lossily.
    Data(String),
    /// The child closed stdout (EOF).
    Eof,
    /// A line exceeded [`MAX_STDIO_LINE_BYTES`].
    TooLong,
    /// An I/O error reading stdout.
    Err(String),
}

/// A bounded, in-memory tail of the child's stderr (kept for the failure message).
#[derive(Default)]
struct StderrTail {
    lines: VecDeque<String>,
}

impl StderrTail {
    fn push(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        self.lines
            .push_back(trimmed.chars().take(MAX_STDERR_LINE_CHARS).collect());
        while self.lines.len() > MAX_STDERR_TAIL_LINES {
            self.lines.pop_front();
        }
    }

    fn rendered(&self) -> String {
        self.lines.iter().cloned().collect::<Vec<_>>().join(" | ")
    }
}

/// A live, spawned managed-stdio MCP child. Owns the child handle + stdin, a channel
/// of stdout lines (filled by a reader thread), and the bounded stderr tail. The
/// child is killed + reaped on [`Drop`], so a managed server never outlives its
/// operation.
struct StdioChild {
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<StdoutLine>,
    stderr_tail: Arc<Mutex<StderrTail>>,
    stderr_handle: Option<JoinHandle<()>>,
    next_id: u64,
}

impl StdioChild {
    /// Spawn `command` with `args` (argv only — never a shell), wire reader threads
    /// over its stdout (lines → channel) and stderr (→ bounded tail), and return the
    /// live child. The child inherits the parent env + cwd unchanged; no extra env or
    /// flag is injected.
    fn spawn(command: &str, args: &[String]) -> Result<Self, McpClientError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| McpClientError::Spawn(format!("{command}: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpClientError::Spawn(format!("{command}: no stdin handle")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpClientError::Spawn(format!("{command}: no stdout handle")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpClientError::Spawn(format!("{command}: no stderr handle")))?;

        // stdout reader thread: bounded line reads → channel. Stops on EOF / error /
        // oversized line, or when the receiver is dropped.
        let (tx, stdout_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_capped_line(&mut reader, MAX_STDIO_LINE_BYTES) {
                    LineRead::Line(bytes) => {
                        let s = String::from_utf8_lossy(&bytes).to_string();
                        if tx.send(StdoutLine::Data(s)).is_err() {
                            break;
                        }
                    }
                    LineRead::Eof => {
                        let _ = tx.send(StdoutLine::Eof);
                        break;
                    }
                    LineRead::TooLong => {
                        let _ = tx.send(StdoutLine::TooLong);
                        break;
                    }
                    LineRead::Err(e) => {
                        let _ = tx.send(StdoutLine::Err(e));
                        break;
                    }
                }
            }
        });

        // stderr reader thread: bounded line reads → bounded tail. Drained so the
        // child never blocks on a full stderr pipe.
        let stderr_tail = Arc::new(Mutex::new(StderrTail::default()));
        let tail = Arc::clone(&stderr_tail);
        let stderr_handle = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            // EOF / oversized / error all end the drain (anything but a complete line).
            while let LineRead::Line(bytes) = read_capped_line(&mut reader, MAX_STDERR_LINE_CHARS * 8)
            {
                let s = String::from_utf8_lossy(&bytes).to_string();
                if let Ok(mut t) = tail.lock() {
                    t.push(&s);
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            stdout_rx,
            stderr_tail,
            stderr_handle: Some(stderr_handle),
            next_id: 1,
        })
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Run the MCP `initialize` handshake, then send the best-effort
    /// `notifications/initialized` (a notification — no id, no response awaited). A
    /// failed handshake is an honest error.
    fn initialize(&mut self, timeout: Duration) -> Result<(), McpClientError> {
        let params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "relux", "version": env!("CARGO_PKG_VERSION") },
        });
        let _ = self.request("initialize", &params, timeout)?;
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        });
        self.write_line(&notif)?;
        Ok(())
    }

    /// Write one JSON value as a single newline-terminated line to the child's stdin.
    fn write_line(&mut self, value: &serde_json::Value) -> Result<(), McpClientError> {
        let mut line = serde_json::to_string(value).map_err(|e| McpClientError::Io(e.to_string()))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| McpClientError::Io(e.to_string()))?;
        self.stdin
            .flush()
            .map_err(|e| McpClientError::Io(e.to_string()))?;
        Ok(())
    }

    /// Send one JSON-RPC `method` request and return its `result`, bounded by
    /// `timeout`. Server→client notifications (no `id`) and non-JSON log noise are
    /// skipped until a response arrives or the deadline passes. A JSON-RPC `error` is
    /// an honest [`McpClientError::ServerError`].
    fn request(
        &mut self,
        method: &str,
        params: &serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, McpClientError> {
        let id = self.next_id();
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_line(&envelope)?;

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(McpClientError::Timeout);
            }
            match self.stdout_rx.recv_timeout(remaining) {
                Ok(StdoutLine::Data(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // A line that does not parse as JSON is server log noise on stdout
                    // (some servers misbehave); skip it rather than fail a valid
                    // response, still bounded by the deadline.
                    let value: serde_json::Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    // A notification (no `id`) is not our response; keep reading.
                    if value.get("id").is_none() {
                        continue;
                    }
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
                    return value.get("result").cloned().ok_or_else(|| {
                        McpClientError::BadResponse("response had no `result` field".to_string())
                    });
                }
                Ok(StdoutLine::Eof) => return Err(McpClientError::ProcessExited),
                Ok(StdoutLine::TooLong) => return Err(McpClientError::StdioLineTooLong),
                Ok(StdoutLine::Err(e)) => return Err(McpClientError::Io(e)),
                Err(RecvTimeoutError::Timeout) => return Err(McpClientError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(McpClientError::ProcessExited),
            }
        }
    }

    /// Fold the child's bounded, secret-redacted stderr tail into `err` so a failed
    /// spawn/handshake/call surfaces *why*. Kills + reaps the child and joins the
    /// stderr drain first (so the tail is complete and deterministic), then renders a
    /// redacted, clamped suffix. Returns `err` unchanged when there is no stderr.
    fn enrich_error(&mut self, err: McpClientError) -> McpClientError {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stderr_handle.take() {
            let _ = handle.join();
        }
        let tail = self
            .stderr_tail
            .lock()
            .map(|t| t.rendered())
            .unwrap_or_default();
        if tail.is_empty() {
            return err;
        }
        let redacted = relux_core::redact_secrets(&tail);
        let bounded: String = redacted.chars().take(MAX_STDERR_TAIL_CHARS).collect();
        McpClientError::Stdio(format!("{err}; stderr: {bounded}"))
    }
}

impl Drop for StdioChild {
    fn drop(&mut self) {
        // Reap the child so a managed server never outlives its operation. Idempotent
        // with `enrich_error` (a second kill/wait on an already-reaped child is fine).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The outcome of one bounded line read.
enum LineRead {
    Line(Vec<u8>),
    Eof,
    TooLong,
    Err(String),
}

/// Read one `\n`-terminated line (the newline is stripped), bounded to `cap` bytes.
/// Returns [`LineRead::TooLong`] if the line would exceed `cap`, [`LineRead::Eof`] at
/// end of stream with no partial bytes, and any trailing partial line at EOF as a
/// final [`LineRead::Line`]. `BufReader` keeps the underlying reads buffered, so the
/// byte-at-a-time loop stays cheap.
fn read_capped_line<R: Read>(reader: &mut R, cap: usize) -> LineRead {
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => {
                return if buf.is_empty() {
                    LineRead::Eof
                } else {
                    LineRead::Line(buf)
                };
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    return LineRead::Line(buf);
                }
                if buf.len() >= cap {
                    return LineRead::TooLong;
                }
                buf.push(byte[0]);
            }
            Err(e) => return LineRead::Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn read_capped_line_splits_on_newline_and_strips_it() {
        let mut r = Cursor::new(b"first\nsecond\n".to_vec());
        match read_capped_line(&mut r, 1024) {
            LineRead::Line(b) => assert_eq!(b, b"first"),
            _ => panic!("expected a line"),
        }
        match read_capped_line(&mut r, 1024) {
            LineRead::Line(b) => assert_eq!(b, b"second"),
            _ => panic!("expected a line"),
        }
        assert!(matches!(read_capped_line(&mut r, 1024), LineRead::Eof));
    }

    #[test]
    fn read_capped_line_bounds_an_unterminated_giant_line() {
        let mut r = Cursor::new(vec![b'x'; 10_000]);
        assert!(matches!(read_capped_line(&mut r, 100), LineRead::TooLong));
    }

    #[test]
    fn read_capped_line_returns_trailing_partial_at_eof() {
        let mut r = Cursor::new(b"tail-no-newline".to_vec());
        match read_capped_line(&mut r, 1024) {
            LineRead::Line(b) => assert_eq!(b, b"tail-no-newline"),
            _ => panic!("expected the trailing partial line"),
        }
    }

    #[test]
    fn stderr_tail_is_bounded_and_drops_blank_lines() {
        let mut t = StderrTail::default();
        t.push("  \n");
        assert!(t.rendered().is_empty(), "blank lines are dropped");
        for i in 0..(MAX_STDERR_TAIL_LINES + 10) {
            t.push(&format!("line {i}\n"));
        }
        assert_eq!(t.lines.len(), MAX_STDERR_TAIL_LINES);
        // The oldest lines were evicted; the newest is kept.
        assert!(t.rendered().contains(&format!("line {}", MAX_STDERR_TAIL_LINES + 9)));
        assert!(!t.rendered().contains("line 0 "));
    }

    #[test]
    fn spawn_failure_is_an_honest_error() {
        // Nothing by this name is on PATH; the spawn must fail clearly (no fake list).
        let err = discover_tools("relux-mcp-no-such-binary-xyzzy", &[], 1_000).unwrap_err();
        assert!(matches!(err, McpClientError::Spawn(_)), "got {err:?}");
    }

    #[test]
    fn an_unsafe_command_is_refused_before_spawning() {
        // A shell-metacharacter command never reaches `Command::spawn`.
        let err = discover_tools("sh;rm -rf /", &[], 1_000).unwrap_err();
        assert!(matches!(err, McpClientError::Spawn(_)), "got {err:?}");
    }
}
