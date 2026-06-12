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
//! ## Process model (two modes: spawn-per-operation OR an operator-managed pool)
//!
//! 1. **Spawn-per-operation (the safe fallback).** [`discover_tools`] / [`call_tool`]
//!    **spawn their own child, run one logical operation (`initialize` → the request),
//!    and reap it** — exactly like the HTTP client opens a fresh `Connection: close`
//!    POST per operation. The child is killed (via [`StdioChild::drop`]) when the
//!    operation ends, so it can never linger between operations.
//! 2. **Operator-managed pool (a real lifecycle).** [`pool`] keeps an operator-started
//!    process **alive** so repeated discovery / invocation reuse ONE `initialize`d
//!    process (start / stop / restart / status, with a redacted log tail). A managed
//!    process is killed + reaped on stop / restart / drop / process shutdown; it never
//!    runs without an explicit operator start. See the "Managed-stdio process pool"
//!    section below and `docs/mcp.md`.
//!
//! Both modes are safe by construction (identical spawn rules):
//!
//! - **No shell.** The command + args are passed as `argv` to [`std::process::Command`]
//!   (re-validated by [`relux_core::validate_stdio_command`] on every spawn). No
//!   string is ever handed to a shell, so there is no metacharacter-injection surface.
//! - **Governed env + cwd (no danger flag).** The child inherits the parent
//!   environment and adds ONLY the operator-configured `env` — already RESOLVED from
//!   the local secret store by the kernel
//!   ([`crate::secret_store::resolve_managed_env_and_cwd`]) into a plaintext
//!   `(name, value)` list that this module hands straight to `Command::env` and never
//!   stores, logs, or returns. An optional `cwd` (already validated INSIDE the safe
//!   workspace root by the kernel) sets the child's working directory. No bypass/danger
//!   flag is ever injected. A spawn with no env/cwd is exactly the prior behavior.
//! - **Bounded.** A per-call timeout bounds every request (the child is killed on
//!   expiry); each stdout line is size-capped ([`MAX_STDIO_LINE_BYTES`]); stderr is
//!   drained into a bounded, secret-redacted tail surfaced only on failure.
//! - **Honest failures.** A spawn failure, EOF before a response, an oversized line,
//!   a malformed body, or a JSON-RPC `error` becomes a clear [`McpClientError`] — a
//!   `tools/call` `isError` is a runtime failure, never a fabricated success.

use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use relux_core::{McpResource, McpResourceContent, McpTool};

use crate::mcp::{
    parse_resources_list, parse_tools_list, shape_resource_read_result, shape_tool_call_result,
    McpClientError,
};

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
    env: &[(String, String)],
    cwd: Option<&Path>,
    timeout_ms: u64,
) -> Result<Vec<McpTool>, McpClientError> {
    run_op(command, args, env, cwd, timeout_ms, |child, timeout| {
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
    env: &[(String, String)],
    cwd: Option<&Path>,
    tool_name: &str,
    arguments: &serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, McpClientError> {
    run_op(command, args, env, cwd, timeout_ms, |child, timeout| {
        let params = serde_json::json!({ "name": tool_name, "arguments": arguments });
        let result = child.request("tools/call", &params, timeout)?;
        shape_tool_call_result(&result)
    })
}

/// Spawn the managed command, run `initialize` → `resources/list`, reap the child, and
/// return the discovered resources (bounded + sanitized) or an honest [`McpClientError`].
///
/// MCP **resources** are a READ-ONLY context surface (files/records/docs an agent can
/// read) — distinct from tools (which act). This performs a bounded subprocess read and
/// mutates nothing on the server. The result reuses the SAME bounding/sanitizing as the
/// HTTP client ([`parse_resources_list`]); only the transport differs.
pub fn list_resources(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
    timeout_ms: u64,
) -> Result<Vec<McpResource>, McpClientError> {
    run_op(command, args, env, cwd, timeout_ms, |child, timeout| {
        let result = child.request("resources/list", &serde_json::json!({}), timeout)?;
        parse_resources_list(&result)
    })
}

/// Spawn the managed command, run `initialize` → `resources/read` for `uri`, reap the
/// child, and return a **shaped, sanitized, secret-redacted** [`McpResourceContent`]
/// (never the raw JSON-RPC envelope, never raw binary bytes) or an honest
/// [`McpClientError`]. A `resources/read` is inert — it performs no action and mutates
/// nothing. `uri` is the caller's responsibility to validate
/// ([`relux_core::is_valid_mcp_resource_uri`]); the result reuses the SAME shaping +
/// redaction as the HTTP client ([`shape_resource_read_result`]).
pub fn read_resource(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
    uri: &str,
    timeout_ms: u64,
) -> Result<McpResourceContent, McpClientError> {
    run_op(command, args, env, cwd, timeout_ms, |child, timeout| {
        let params = serde_json::json!({ "uri": uri });
        let result = child.request("resources/read", &params, timeout)?;
        shape_resource_read_result(&result, uri)
    })
}

/// Spawn the managed command, initialize it, run `op` (one `tools/list`, `tools/call`,
/// `resources/list`, or `resources/read`), then reap the child. On any failure the
/// error is **enriched** with the child's bounded, secret-redacted stderr tail so the
/// operator can see why.
fn run_op<T>(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
    timeout_ms: u64,
    op: impl FnOnce(&mut StdioChild, Duration) -> Result<T, McpClientError>,
) -> Result<T, McpClientError> {
    // Re-validate the command on every spawn (defense in depth) so a never-shelled,
    // bounded argv is the only thing we ever hand to the OS.
    relux_core::validate_stdio_command(command, args)
        .map_err(|e| McpClientError::Spawn(e.to_string()))?;
    let timeout = Duration::from_millis(timeout_ms);
    let mut child = StdioChild::spawn(command, args, env, cwd)?;
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

/// Build the `Command` for a managed-stdio spawn: argv only (never a shell), the
/// child's stdio piped, the parent env inherited PLUS the resolved `env` entries, and
/// the validated `cwd` when one is given. Factored out (and tested) so the exact env
/// the OS will hand the child is observable without spawning a process.
fn build_command(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(command);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd
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
    /// live child. The child inherits the parent env plus the resolved `env` entries
    /// (already secret-resolved by the kernel — handed to `Command::env` and never
    /// stored/logged here), and runs in `cwd` when one is given (already validated
    /// inside the safe workspace root). No bypass/danger flag is injected.
    fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&Path>,
    ) -> Result<Self, McpClientError> {
        let mut cmd = build_command(command, args, env, cwd);
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
                    // A response whose `id` does not match THIS request's id is a
                    // stale reply (e.g. a previous request that timed out and answered
                    // late) — skip it too, so a reused process never confuses one
                    // request's response with another. Ids are monotonically
                    // increasing, so only the exact match is ours.
                    match value.get("id").and_then(|v| v.as_u64()) {
                        Some(resp_id) if resp_id == id => {}
                        // Has an id but not ours → stale; or a non-integer id we did
                        // not send → not ours. Either way, keep draining.
                        _ => continue,
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

    /// The OS process id of this child (safe to surface — carries no secret).
    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// A bounded, secret-redacted snapshot of the child's stderr tail, one line per
    /// entry (most recent last). Used for the managed-pool status log tail. Reads the
    /// shared tail under its own short lock; never blocks on the child itself.
    fn redacted_log_lines(&self) -> Vec<String> {
        let lines: Vec<String> = self
            .stderr_tail
            .lock()
            .map(|t| t.lines.iter().cloned().collect())
            .unwrap_or_default();
        lines
            .into_iter()
            .map(|l| relux_core::redact_secrets(&l))
            .collect()
    }

    /// If the child has already exited, return its (lossy) status string; otherwise
    /// `None`. Non-blocking (`try_wait`), so a status read can cheaply detect a crash.
    fn poll_exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(format!("process exited: {status}")),
            // Still running, or an error querying — treat the latter as "unknown,
            // assume alive" (a later request will surface a real transport failure).
            _ => None,
        }
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

// ===========================================================================
// Managed-stdio process pool (operator-visible lifecycle: start/stop/restart,
// status, reuse for tools/list + tools/call).
// ===========================================================================
//
// The spawn-per-operation client above stays the safe FALLBACK. On top of it,
// the pool keeps an operator-started child **alive** so repeated discovery /
// invocation reuse ONE `initialize`d process instead of paying spawn + handshake
// per call (Hermes keeps the MCP client connected between `list_tools`/`call_tool`;
// `_probe_single_server` connects → lists → disconnects only for a one-shot probe —
// `reference/hermes-agent-main/hermes_cli/mcp_config.py`). It stays safe by
// construction: same argv-only spawn, no env / no `cwd`, no bypass flag, bounded
// logs/memory, and the child is killed + reaped on stop / restart / drop / process
// shutdown.
//
// Concurrency model: the pool maps `server id → ManagedEntry`. Each entry serializes
// requests to its OWN process behind a `child` mutex (one JSON-RPC exchange at a
// time), so two different servers run in parallel but two calls to the SAME server
// queue. The lightweight status fields (state / pid / started-at) are atomics, so a
// status read never blocks a live request and can observe `Starting` mid-spawn.

/// State encodings for [`ManagedEntry::state`] (an `AtomicU8`).
const STATE_STOPPED: u8 = 0;
const STATE_STARTING: u8 = 1;
const STATE_RUNNING: u8 = 2;
const STATE_FAILED: u8 = 3;

fn state_from_u8(v: u8) -> relux_core::ManagedStdioState {
    match v {
        STATE_STARTING => relux_core::ManagedStdioState::Starting,
        STATE_RUNNING => relux_core::ManagedStdioState::Running,
        STATE_FAILED => relux_core::ManagedStdioState::Failed,
        _ => relux_core::ManagedStdioState::Stopped,
    }
}

/// Wall-clock epoch milliseconds (best effort; `0` if the clock is before the epoch).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Keep only the most recent [`relux_core::MAX_MANAGED_STDIO_LOG_LINES`] lines.
fn bound_log(mut lines: Vec<String>) -> Vec<String> {
    let max = relux_core::MAX_MANAGED_STDIO_LOG_LINES;
    if lines.len() > max {
        lines = lines.split_off(lines.len() - max);
    }
    lines
}

/// Secret-redact + bound a failure message stored on an entry.
fn redact_and_bound(s: &str) -> String {
    let r = relux_core::redact_secrets(s);
    r.chars().take(MAX_STDERR_TAIL_CHARS).collect()
}

/// Whether an error means the managed process is suspect and must be torn down (it
/// died, the read timed out — the response stream may be desynced — or a low-level
/// transport/spawn failure). An *application* error (a JSON-RPC `error`, or a
/// `tools/call` `isError`) leaves the process healthy and reusable, so it is NOT
/// fatal: the call surfaces honestly without killing the daemon.
fn is_fatal_transport_error(err: &McpClientError) -> bool {
    matches!(
        err,
        McpClientError::ProcessExited
            | McpClientError::StdioLineTooLong
            | McpClientError::Timeout
            | McpClientError::Io(_)
            | McpClientError::Spawn(_)
            | McpClientError::Stdio(_)
    )
}

/// The non-process metadata tracked for a managed entry (everything that is not a
/// cheap atomic). Guarded by a short-lived mutex never held across a spawn.
#[derive(Default)]
struct EntryMeta {
    /// The honest, redacted reason for the last failure (spawn/handshake/transport).
    last_error: Option<String>,
    /// Tools discovered by the last live `tools/list` against the running process.
    tools_count: Option<usize>,
    /// A redacted log-tail snapshot captured at stop / failure, so logs survive after
    /// the child is dropped. While running, the live tail is read from the child.
    log_tail: Vec<String>,
}

/// One managed server's lifecycle slot: the live process (behind `child`), the
/// lock-free status atomics, and the metadata.
struct ManagedEntry {
    id: String,
    /// One of the `STATE_*` constants.
    state: AtomicU8,
    /// The running child's OS pid, or `0` when none.
    pid: AtomicU32,
    /// Epoch-millis the current process started, or `0` when none.
    started_at_ms: AtomicU64,
    meta: Mutex<EntryMeta>,
    /// The live process, or `None` when stopped / failed. Locked for the duration of
    /// one JSON-RPC exchange (and the spawn), serializing calls to this one server.
    child: Mutex<Option<StdioChild>>,
}

impl ManagedEntry {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            state: AtomicU8::new(STATE_STOPPED),
            pid: AtomicU32::new(0),
            started_at_ms: AtomicU64::new(0),
            meta: Mutex::new(EntryMeta::default()),
            child: Mutex::new(None),
        }
    }

    fn meta(&self) -> std::sync::MutexGuard<'_, EntryMeta> {
        self.meta.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn child_guard(&self) -> std::sync::MutexGuard<'_, Option<StdioChild>> {
        self.child.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record a failure: state → `Failed`, clear pid/started-at, store the reason.
    fn fail(&self, message: &str) {
        self.state.store(STATE_FAILED, Ordering::SeqCst);
        self.pid.store(0, Ordering::SeqCst);
        self.started_at_ms.store(0, Ordering::SeqCst);
        self.meta().last_error = Some(redact_and_bound(message));
    }

    /// Spawn `command` + `args` (argv only) and run the `initialize` handshake,
    /// replacing any existing process. Sets `Starting` for the spawn window, then
    /// `Running` (with pid + started-at) on success or `Failed` (with the redacted,
    /// stderr-enriched reason) on failure.
    fn start(
        &self,
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&Path>,
        timeout_ms: u64,
    ) {
        self.state.store(STATE_STARTING, Ordering::SeqCst);
        let timeout = Duration::from_millis(timeout_ms);
        let mut guard = self.child_guard();
        // Drop (kill + reap) any prior process first.
        *guard = None;
        self.pid.store(0, Ordering::SeqCst);
        self.started_at_ms.store(0, Ordering::SeqCst);
        match StdioChild::spawn(command, args, env, cwd) {
            Ok(mut child) => match child.initialize(timeout) {
                Ok(()) => {
                    self.pid.store(child.pid(), Ordering::SeqCst);
                    self.started_at_ms.store(now_ms(), Ordering::SeqCst);
                    {
                        let mut m = self.meta();
                        m.last_error = None;
                        m.tools_count = None;
                        m.log_tail.clear();
                    }
                    self.state.store(STATE_RUNNING, Ordering::SeqCst);
                    *guard = Some(child);
                }
                Err(err) => {
                    // Fold the child's stderr tail into the reason, then reap it.
                    let enriched = child.enrich_error(err);
                    *guard = None;
                    self.fail(&enriched.to_string());
                }
            },
            Err(err) => {
                *guard = None;
                self.fail(&err.to_string());
            }
        }
    }

    /// Kill + reap the running process (if any), capturing its final redacted log
    /// tail, and set state → `Stopped`. Idempotent.
    fn stop(&self) {
        let mut guard = self.child_guard();
        if let Some(child) = guard.as_ref() {
            let tail = bound_log(child.redacted_log_lines());
            self.meta().log_tail = tail;
        }
        *guard = None; // Drop → kill + reap.
        self.state.store(STATE_STOPPED, Ordering::SeqCst);
        self.pid.store(0, Ordering::SeqCst);
        self.started_at_ms.store(0, Ordering::SeqCst);
        {
            let mut m = self.meta();
            m.last_error = None;
            m.tools_count = None;
        }
    }

    /// Best-effort crash detection: if the running child has already exited, mark it
    /// `Failed` (with the exit reason + final log tail). Uses `try_lock` so a status
    /// read never blocks a live request — if the child is busy, it is by definition
    /// still running.
    fn detect_crash(&self) {
        if self.state.load(Ordering::SeqCst) != STATE_RUNNING {
            return;
        }
        if let Ok(mut guard) = self.child.try_lock() {
            let crashed = guard.as_mut().and_then(|c| {
                c.poll_exited()
                    .map(|reason| (reason, bound_log(c.redacted_log_lines())))
            });
            if let Some((reason, tail)) = crashed {
                *guard = None;
                drop(guard);
                self.fail(&reason);
                self.meta().log_tail = tail;
            }
        }
    }

    fn is_running(&self) -> bool {
        self.detect_crash();
        self.state.load(Ordering::SeqCst) == STATE_RUNNING
    }

    /// Send one JSON-RPC request to the live process and return its raw `result`.
    /// A fatal transport error tears the process down (drop + reap), records the
    /// honest reason, and marks `Failed`; an application error leaves it reusable.
    fn request_reuse(
        &self,
        method: &str,
        params: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, McpClientError> {
        let timeout = Duration::from_millis(timeout_ms);
        let mut guard = self.child_guard();
        // Borrow the child only for the exchange; end the borrow before mutating the
        // entry on a fatal error.
        let (result, tail) = {
            let Some(child) = guard.as_mut() else {
                return Err(McpClientError::ProcessExited);
            };
            let r = child.request(method, params, timeout);
            let tail = bound_log(child.redacted_log_lines());
            (r, tail)
        };
        match result {
            Ok(v) => {
                // Keep the latest live tail visible to status while running.
                self.meta().log_tail = tail;
                Ok(v)
            }
            Err(err) if is_fatal_transport_error(&err) => {
                let enriched = match guard.as_mut() {
                    Some(child) => child.enrich_error(err),
                    None => err,
                };
                *guard = None;
                drop(guard);
                self.fail(&enriched.to_string());
                self.meta().log_tail = tail;
                Err(enriched)
            }
            Err(err) => Err(err),
        }
    }

    /// A status snapshot. Reads the cheap atomics, detects a crash first, and prefers
    /// the live stderr tail when running (falling back to the captured snapshot).
    fn status(&self) -> relux_core::ManagedStdioStatus {
        self.detect_crash();
        let state = state_from_u8(self.state.load(Ordering::SeqCst));
        let pid = match self.pid.load(Ordering::SeqCst) {
            0 => None,
            p => Some(p),
        };
        let started_at_ms = match self.started_at_ms.load(Ordering::SeqCst) {
            0 => None,
            t => Some(t),
        };
        let (last_error, tools_count, snapshot_tail) = {
            let m = self.meta();
            (m.last_error.clone(), m.tools_count, m.log_tail.clone())
        };
        let log_tail = if state == relux_core::ManagedStdioState::Running {
            // Non-blocking live read; fall back to the snapshot if the child is busy.
            match self.child.try_lock() {
                Ok(guard) => match guard.as_ref() {
                    Some(child) => bound_log(child.redacted_log_lines()),
                    None => snapshot_tail,
                },
                Err(_) => snapshot_tail,
            }
        } else {
            snapshot_tail
        };
        relux_core::ManagedStdioStatus {
            id: self.id.clone(),
            state,
            pid,
            started_at_ms,
            last_error,
            tools_count,
            log_tail,
        }
    }
}

/// A process-global pool of managed-stdio MCP servers. Accessed via [`pool`].
///
/// Lives OUTSIDE the serializable [`crate::state::KernelState`] (a live OS process is
/// not snapshot state): the kernel's registry stays the source of truth for *what* is
/// registered; this pool owns *whether it is running*. The kernel drives it
/// (start/stop/restart/status) and reuses a running process for discovery/invocation.
pub struct ManagedPool {
    entries: Mutex<HashMap<String, Arc<ManagedEntry>>>,
}

/// The process-global managed-stdio pool. Created lazily on first use.
static POOL: OnceLock<ManagedPool> = OnceLock::new();

/// The process-global managed-stdio pool.
pub fn pool() -> &'static ManagedPool {
    POOL.get_or_init(ManagedPool::new)
}

impl ManagedPool {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<ManagedEntry>>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Get or create the entry for `id`.
    fn entry(&self, id: &str) -> Arc<ManagedEntry> {
        self.entries()
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(ManagedEntry::new(id)))
            .clone()
    }

    /// The entry for `id`, if the pool is tracking one.
    fn lookup(&self, id: &str) -> Option<Arc<ManagedEntry>> {
        self.entries().get(id).cloned()
    }

    /// Start (or replace) the managed process for `id`. Returns its status — `Running`
    /// on success, `Failed` (with `last_error`) if the spawn/handshake failed.
    pub fn start(
        &self,
        id: &str,
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&Path>,
        timeout_ms: u64,
    ) -> relux_core::ManagedStdioStatus {
        let entry = self.entry(id);
        entry.start(command, args, env, cwd, timeout_ms);
        entry.status()
    }

    /// Record a `failed` status for `id` WITHOUT spawning — used when the kernel cannot
    /// resolve a managed-stdio server's env secrets / cwd before a start, so the
    /// operator sees an honest failure (naming the missing secret KEY, never a value)
    /// instead of a fabricated `running`. Reaps any prior process first.
    pub fn fail(&self, id: &str, reason: &str) -> relux_core::ManagedStdioStatus {
        let entry = self.entry(id);
        {
            // Drop any running process before marking failed.
            let mut guard = entry.child_guard();
            *guard = None;
        }
        entry.fail(reason);
        entry.status()
    }

    /// Stop the managed process for `id` (kill + reap). Idempotent — a never-started /
    /// already-stopped server returns a clean `Stopped` status.
    pub fn stop(&self, id: &str) -> relux_core::ManagedStdioStatus {
        let entry = self.entry(id);
        entry.stop();
        entry.status()
    }

    /// Restart: stop the current process then start a fresh one.
    pub fn restart(
        &self,
        id: &str,
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&Path>,
        timeout_ms: u64,
    ) -> relux_core::ManagedStdioStatus {
        let entry = self.entry(id);
        entry.stop();
        entry.start(command, args, env, cwd, timeout_ms);
        entry.status()
    }

    /// The current status for `id` (a clean `Stopped` default when untracked).
    pub fn status(&self, id: &str) -> relux_core::ManagedStdioStatus {
        match self.lookup(id) {
            Some(e) => e.status(),
            None => relux_core::ManagedStdioStatus::stopped(id),
        }
    }

    /// Whether a live, running process exists for `id` (with crash detection).
    pub fn is_running(&self, id: &str) -> bool {
        self.lookup(id).map(|e| e.is_running()).unwrap_or(false)
    }

    /// Reuse the running process to run `tools/list`. Errors with
    /// [`McpClientError::ProcessExited`] when no process is running (the caller then
    /// decides whether to fall back to a spawn-per-operation discovery).
    pub fn list_tools(
        &self,
        id: &str,
        timeout_ms: u64,
    ) -> Result<Vec<relux_core::McpTool>, McpClientError> {
        let entry = self.lookup(id).ok_or(McpClientError::ProcessExited)?;
        let result = entry.request_reuse("tools/list", &serde_json::json!({}), timeout_ms)?;
        let tools = parse_tools_list(&result)?;
        entry.meta().tools_count = Some(tools.len());
        Ok(tools)
    }

    /// Reuse the running process to run `tools/call`, returning the SHAPED result
    /// (never the raw envelope). Errors with [`McpClientError::ProcessExited`] when no
    /// process is running.
    pub fn call_tool(
        &self,
        id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, McpClientError> {
        let entry = self.lookup(id).ok_or(McpClientError::ProcessExited)?;
        let params = serde_json::json!({ "name": tool_name, "arguments": arguments });
        let result = entry.request_reuse("tools/call", &params, timeout_ms)?;
        shape_tool_call_result(&result)
    }

    /// Reuse the running process to run `resources/list` (READ-ONLY context). Errors
    /// with [`McpClientError::ProcessExited`] when no process is running (the caller then
    /// decides whether to fall back to a spawn-per-operation listing).
    pub fn list_resources(
        &self,
        id: &str,
        timeout_ms: u64,
    ) -> Result<Vec<McpResource>, McpClientError> {
        let entry = self.lookup(id).ok_or(McpClientError::ProcessExited)?;
        let result = entry.request_reuse("resources/list", &serde_json::json!({}), timeout_ms)?;
        parse_resources_list(&result)
    }

    /// Reuse the running process to run `resources/read` for `uri`, returning the
    /// SHAPED, sanitized, secret-redacted content (never the raw envelope, never raw
    /// bytes). A `resources/read` is inert. Errors with
    /// [`McpClientError::ProcessExited`] when no process is running.
    pub fn read_resource(
        &self,
        id: &str,
        uri: &str,
        timeout_ms: u64,
    ) -> Result<McpResourceContent, McpClientError> {
        let entry = self.lookup(id).ok_or(McpClientError::ProcessExited)?;
        let params = serde_json::json!({ "uri": uri });
        let result = entry.request_reuse("resources/read", &params, timeout_ms)?;
        shape_resource_read_result(&result, uri)
    }

    /// Stop and forget `id` entirely (used when its registration is removed).
    pub fn remove(&self, id: &str) {
        let removed = self.entries().remove(id);
        if let Some(entry) = removed {
            entry.stop();
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
        let err =
            discover_tools("relux-mcp-no-such-binary-xyzzy", &[], &[], None, 1_000).unwrap_err();
        assert!(matches!(err, McpClientError::Spawn(_)), "got {err:?}");
    }

    #[test]
    fn an_unsafe_command_is_refused_before_spawning() {
        // A shell-metacharacter command never reaches `Command::spawn`.
        let err = discover_tools("sh;rm -rf /", &[], &[], None, 1_000).unwrap_err();
        assert!(matches!(err, McpClientError::Spawn(_)), "got {err:?}");
    }

    #[test]
    fn build_command_applies_resolved_env_and_cwd() {
        // The exact env the OS will hand the child is observable on the built Command,
        // without spawning. This proves the resolved secret value IS injected into the
        // child environment (and only that env — `get_envs` returns only what we add).
        let tmp = tempfile::tempdir().unwrap();
        let env = vec![("MY_TOKEN".to_string(), "resolved-secret-1234".to_string())];
        let cmd = build_command("node", &["-e".to_string()], &env, Some(tmp.path()));
        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();
        assert!(
            envs.contains(&("MY_TOKEN".to_string(), Some("resolved-secret-1234".to_string()))),
            "resolved secret not injected into child env: {envs:?}"
        );
        assert_eq!(cmd.get_current_dir(), Some(tmp.path()));
    }

    /// End-to-end proof the spawned process RECEIVES the injected env var — it exits 7
    /// IFF the var is defined, and prints NOTHING (no value, no fixture output). Uses
    /// the same `build_command` builder Relux spawns through. Platform-gated to a shell
    /// that is always present (this is the TEST harness invoking it, not Relux running a
    /// shell — Relux still spawns argv-only).
    #[cfg(unix)]
    #[test]
    fn spawned_child_receives_injected_env_secret_unix() {
        let env = vec![("RELUX_ENV_PROOF".to_string(), "must-not-print".to_string())];
        let mut cmd = build_command(
            "sh",
            &[
                "-c".to_string(),
                "[ -n \"$RELUX_ENV_PROOF\" ] && exit 7 || exit 3".to_string(),
            ],
            &env,
            None,
        );
        // Inherit stdio for this raw proof (no MCP handshake involved).
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let status = cmd.status().expect("spawn sh");
        assert_eq!(status.code(), Some(7), "child did not see the injected env var");
    }

    #[cfg(windows)]
    #[test]
    fn spawned_child_receives_injected_env_secret_windows() {
        let env = vec![("RELUX_ENV_PROOF".to_string(), "must-not-print".to_string())];
        let mut cmd = build_command(
            "cmd",
            &[
                "/c".to_string(),
                "if defined RELUX_ENV_PROOF (exit 7) else (exit 3)".to_string(),
            ],
            &env,
            None,
        );
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let status = cmd.status().expect("spawn cmd");
        assert_eq!(status.code(), Some(7), "child did not see the injected env var");
    }
}
