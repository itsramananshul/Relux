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
