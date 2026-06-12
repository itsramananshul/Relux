//! Integration tests for the **managed-stdio MCP client** against a REAL subprocess
//! (`src/bin/relux_mcp_test_server.rs`, a tiny pure-Rust MCP stdio server) — not the
//! kernel's built-in echo. They prove the real lifecycle end to end: spawn →
//! `initialize` → `tools/list` / `tools/call` → reap, plus honest failures and the
//! kernel-level dispatch through `KernelState::discover_mcp_tools`.

use relux_kernel::mcp::McpClientError;
use relux_kernel::mcp_stdio;
use relux_kernel::state::KernelState;

/// Path to the test-fixture MCP stdio server binary (built by Cargo for this test).
fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_relux_mcp_test_server")
}

#[test]
fn discovers_tools_from_a_real_stdio_server() {
    let tools = mcp_stdio::discover_tools(fixture(), &[], 5_000).expect("discovery ok");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"status.summary"), "tools: {names:?}");
    assert!(names.contains(&"boom"), "tools: {names:?}");
    assert!(names.contains(&"noisy"), "tools: {names:?}");
    // The description is carried + sanitized.
    let status = tools.iter().find(|t| t.name == "status.summary").unwrap();
    assert!(status.description.contains("status summary"), "desc: {}", status.description);
}

#[test]
fn calls_a_tool_and_gets_a_shaped_result() {
    let out = mcp_stdio::call_tool(
        fixture(),
        &[],
        "status.summary",
        &serde_json::json!({ "q": "hi" }),
        5_000,
    )
    .expect("call ok");
    // Shaped result — NEVER the raw { jsonrpc, id, result } envelope.
    assert_eq!(out["result"], "status ok; q=hi");
    assert_eq!(out["structuredContent"], serde_json::json!({ "ok": true }));
    assert!(out.get("jsonrpc").is_none(), "raw envelope leaked: {out}");
}

#[test]
fn a_tool_iserror_is_an_honest_failure_not_a_fake_success() {
    let err = mcp_stdio::call_tool(fixture(), &[], "boom", &serde_json::json!({}), 5_000)
        .unwrap_err();
    assert!(
        matches!(err, McpClientError::ToolCallError(ref m) if m.contains("intentional failure")),
        "got {err:?}"
    );
}

#[test]
fn a_noisy_tool_still_returns_ok() {
    // The server writes to stderr then returns ok; the client drains stderr (bounded)
    // and still surfaces the shaped ok result.
    let out = mcp_stdio::call_tool(fixture(), &[], "noisy", &serde_json::json!({}), 5_000)
        .expect("call ok");
    assert_eq!(out["result"], "noisy ok");
}

#[test]
fn an_unknown_tool_fails_cleanly() {
    let err = mcp_stdio::call_tool(fixture(), &[], "does.not.exist", &serde_json::json!({}), 5_000)
        .unwrap_err();
    // A JSON-RPC error from the server, surfaced honestly (never a fabricated result).
    assert!(matches!(err, McpClientError::ServerError { .. }), "got {err:?}");
}

#[test]
fn spawn_failure_is_honest() {
    let err = mcp_stdio::discover_tools("relux-mcp-no-such-binary-xyzzy", &[], 1_000).unwrap_err();
    assert!(matches!(err, McpClientError::Spawn(_)), "got {err:?}");
}

#[test]
fn kernel_discovers_a_managed_stdio_server_through_the_registry() {
    let mut k = KernelState::new();
    // Registration is explicit + validated; it does NOT spawn the command.
    k.register_mcp_stdio_server("local-fs", fixture(), &[], "test stdio server", true, Some(5_000))
        .expect("register ok");

    // Discover runs the live tools/list by SPAWNING the command (operator-controlled).
    let tools = k.discover_mcp_tools("local-fs").expect("discovery ok");
    let status = tools
        .iter()
        .find(|t| t.tool_name == "status.summary")
        .expect("status.summary discovered");
    // Mapped into the standard ToolDescriptor shape, under the mcp:<server> namespace.
    assert_eq!(status.plugin_id, "mcp:local-fs");
    assert_eq!(status.permission, "tool:mcp-local-fs:summary");
    assert_eq!(status.source_kind, "Mcp");
    // Unclassified ⇒ gated (needs approval), never auto-runnable.
    assert_eq!(status.executable, relux_core::ToolExecutability::NeedsApproval);
}

#[test]
fn kernel_disabled_stdio_server_refuses_discovery() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server("off", fixture(), &[], "disabled", false, Some(5_000))
        .expect("register ok");
    assert!(k.discover_mcp_tools("off").is_err(), "a disabled server must refuse discovery");
}

// --- Managed-stdio process pool lifecycle (start/stop/restart/reuse/crash) ---

use relux_core::ManagedStdioState;
use relux_kernel::mcp_stdio::pool;

/// Parse the per-process `calls` counter the fixture's `whoami` tool returns in its
/// structuredContent, proving a SINGLE process is reused across calls.
fn calls_from_whoami(out: &serde_json::Value) -> u64 {
    out["structuredContent"]["calls"].as_u64().expect("calls counter")
}
fn pid_from_whoami(out: &serde_json::Value) -> u64 {
    out["structuredContent"]["pid"].as_u64().expect("pid")
}

#[test]
fn pool_start_status_list_call_stop_lifecycle() {
    let id = "pool-lifecycle";
    let status = pool().start(id, fixture(), &[], 5_000);
    assert_eq!(status.state, ManagedStdioState::Running, "start → running: {status:?}");
    assert!(status.pid.is_some(), "a running process has a pid");
    assert!(status.started_at_ms.is_some(), "a running process has a start time");

    // tools/list reuses the running process.
    let tools = pool().list_tools(id, 5_000).expect("list ok");
    assert!(tools.iter().any(|t| t.name == "whoami"), "tools: {tools:?}");
    // tools_count is now recorded on the status.
    assert_eq!(pool().status(id).tools_count, Some(tools.len()));

    // tools/call reuses the same process and returns a shaped result.
    let out = pool()
        .call_tool(id, "status.summary", &serde_json::json!({ "q": "hi" }), 5_000)
        .expect("call ok");
    assert_eq!(out["result"], "status ok; q=hi");
    assert!(out.get("jsonrpc").is_none(), "raw envelope leaked: {out}");

    let stopped = pool().stop(id);
    assert_eq!(stopped.state, ManagedStdioState::Stopped);
    assert!(stopped.pid.is_none(), "a stopped process has no pid");
    assert!(!pool().is_running(id), "stopped server is not running");
}

#[test]
fn pool_reuses_one_process_across_calls() {
    let id = "pool-reuse";
    let start = pool().start(id, fixture(), &[], 5_000);
    let started_pid = start.pid.expect("running pid");

    let first = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    let second = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    // Same OS process (same pid as the status) and a monotonically increasing
    // per-process counter prove ONE long-lived process served both calls.
    assert_eq!(pid_from_whoami(&first), started_pid as u64);
    assert_eq!(pid_from_whoami(&second), started_pid as u64);
    assert_eq!(calls_from_whoami(&first), 1, "first call on a fresh process");
    assert_eq!(calls_from_whoami(&second), 2, "second call on the SAME process");

    pool().stop(id);
}

#[test]
fn pool_reuse_requires_an_explicit_start() {
    let id = "pool-not-started";
    // No start → no running process → reuse fails cleanly (the caller falls back to
    // spawn-per-operation). Nothing is auto-started.
    assert!(!pool().is_running(id));
    let err = pool().call_tool(id, "whoami", &serde_json::json!({}), 2_000).unwrap_err();
    assert!(matches!(err, McpClientError::ProcessExited), "got {err:?}");
}

#[test]
fn pool_restart_replaces_the_process() {
    let id = "pool-restart";
    let first = pool().start(id, fixture(), &[], 5_000);
    let pid1 = first.pid.expect("pid1");
    // whoami once on the first process.
    let a = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    assert_eq!(calls_from_whoami(&a), 1);

    let second = pool().restart(id, fixture(), &[], 5_000);
    let pid2 = second.pid.expect("pid2");
    assert_ne!(pid1, pid2, "restart spawns a NEW process (different pid)");
    // The fresh process's counter starts over at 1 (it is genuinely a new process).
    let b = pool().call_tool(id, "whoami", &serde_json::json!({}), 5_000).expect("ok");
    assert_eq!(calls_from_whoami(&b), 1, "the restarted process is fresh");

    pool().stop(id);
}

#[test]
fn pool_process_crash_marks_failed_and_records_error() {
    let id = "pool-crash";
    let start = pool().start(id, fixture(), &[], 5_000);
    assert_eq!(start.state, ManagedStdioState::Running);

    // The `crash` tool exits the process without responding; the call sees EOF and
    // fails honestly (never a fabricated success).
    let err = pool().call_tool(id, "crash", &serde_json::json!({}), 5_000).unwrap_err();
    assert!(
        matches!(err, McpClientError::ProcessExited | McpClientError::Stdio(_)),
        "got {err:?}"
    );
    // The status now reports the process as failed, with an honest reason and no pid.
    let status = pool().status(id);
    assert_eq!(status.state, ManagedStdioState::Failed, "crash → failed: {status:?}");
    assert!(status.pid.is_none(), "a dead process has no pid");
    assert!(status.last_error.is_some(), "a crash records an honest reason");
    assert!(!pool().is_running(id));

    pool().stop(id);
}

#[test]
fn pool_start_failure_is_an_honest_failed_status() {
    let id = "pool-bad-binary";
    // Nothing by this name is on PATH; the start fails → a `failed` status with a
    // redacted reason (never a fabricated `running`).
    let status = pool().start(id, "relux-mcp-no-such-binary-xyzzy", &[], 1_000);
    assert_eq!(status.state, ManagedStdioState::Failed, "bad binary → failed: {status:?}");
    assert!(status.last_error.is_some(), "a failed start records why");
    assert!(status.pid.is_none());
}

#[test]
fn kernel_managed_stdio_lifecycle_through_the_registry() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server("life-fs", fixture(), &[], "test stdio server", true, Some(5_000))
        .expect("register ok");

    // Start through the kernel (audited); the process comes up.
    let status = k.start_mcp_stdio_server("life-fs").expect("start ok");
    assert_eq!(status.state, ManagedStdioState::Running);

    // Discovery now REUSES the running process (still mapped into ToolDescriptors).
    let tools = k.discover_mcp_tools("life-fs").expect("discover ok");
    assert!(tools.iter().any(|t| t.tool_name == "whoami"), "tools: {tools:?}");

    // The status surface lists it as running.
    let listed = k.mcp_stdio_statuses();
    assert!(listed.iter().any(|s| s.id == "life-fs" && s.state == ManagedStdioState::Running));

    // Stop through the kernel.
    let stopped = k.stop_mcp_stdio_server("life-fs").expect("stop ok");
    assert_eq!(stopped.state, ManagedStdioState::Stopped);
}

#[test]
fn kernel_lifecycle_refuses_http_and_disabled_servers() {
    let mut k = KernelState::new();
    // An HTTP-loopback server has no process lifecycle.
    k.register_mcp_server("http-one", "http://127.0.0.1:8000/mcp", "http", true, Some(5_000))
        .expect("register http ok");
    assert!(matches!(
        k.start_mcp_stdio_server("http-one"),
        Err(relux_kernel::KernelError::NotAManagedStdioServer(_))
    ));

    // A disabled managed-stdio server refuses to start (enable it first).
    k.register_mcp_stdio_server("off-stdio", fixture(), &[], "disabled", false, Some(5_000))
        .expect("register ok");
    assert!(matches!(
        k.start_mcp_stdio_server("off-stdio"),
        Err(relux_kernel::KernelError::McpServerDisabled(_))
    ));

    // An unknown server is a clean not-found.
    assert!(matches!(
        k.start_mcp_stdio_server("nope"),
        Err(relux_kernel::KernelError::UnknownMcpServer(_))
    ));
}

#[test]
fn kernel_remove_stops_the_managed_process() {
    let mut k = KernelState::new();
    k.register_mcp_stdio_server("rm-fs", fixture(), &[], "to remove", true, Some(5_000))
        .expect("register ok");
    let status = k.start_mcp_stdio_server("rm-fs").expect("start ok");
    assert_eq!(status.state, ManagedStdioState::Running);
    // Removing the registration stops + reaps the process so no daemon lingers.
    k.remove_mcp_server("rm-fs").expect("remove ok");
    assert!(!pool().is_running("rm-fs"), "removed server's process is stopped");
}
